import atexit
import json
import logging
import os
import queue
import subprocess
import threading
import time
from pathlib import Path

PROTOCOL = 4
ENTRY = "hermes"
IDLE_ENV = "LOOM_HERMES_IDLE"
EFFORT_ENV = "LOOM_HERMES_EFFORT"
STOP = "loom-stop"
INJECT_TRIES = 600
INJECT_PAUSE_S = 0.1
CALL_LIMIT_S = 10
SKILLS = Path(__file__).resolve().parent.parent / "skills"

log = logging.getLogger("loom")


class Hook:
    def __init__(self):
        self.lock = threading.Lock()
        self.child = None
        self.pending = {}
        self.next = 0
        atexit.register(self.close)

    def open(self):
        child = subprocess.Popen(
            ["loom", "hook", ENTRY],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        threading.Thread(target=self.read, args=(child,), daemon=True).start()
        threading.Thread(target=self.errors, args=(child,), daemon=True).start()
        return child

    def read(self, child):
        for line in child.stdout:
            try:
                reply = json.loads(line)
            except ValueError:
                continue
            with self.lock:
                pending = self.pending.pop(reply.get("id"), None)
                slot = pending[1] if pending else None
            if slot is None:
                continue
            if reply.get("error"):
                log.warning("loom hook %s: %s", ENTRY, reply["error"])
                reply = None
            slot.put(reply)
        with self.lock:
            if self.child is child:
                self.child = None
            ended = [slot for owner, slot in self.pending.values() if owner is child]
            self.pending = {id: item for id, item in self.pending.items() if item[0] is not child}
        for slot in ended:
            slot.put(None)

    def errors(self, child):
        text = child.stderr.read().strip()
        if child.wait() != 0 and text:
            log.warning("loom hook %s: %s", ENTRY, text)

    def send(self, event):
        slot = queue.Queue(maxsize=1)
        with self.lock:
            self.next += 1
            id = self.next
            try:
                if self.child is None or self.child.poll() is not None:
                    self.child = self.open()
                self.pending[id] = (self.child, slot)
                self.child.stdin.write(json.dumps({"protocol": PROTOCOL, "id": id, **event}) + "\n")
                self.child.stdin.flush()
            except OSError as e:
                log.warning("loom hook %s: %s", ENTRY, e)
                self.pending.pop(id, None)
                slot.put(None)
        return id, slot

    def call(self, event):
        id, slot = self.send(event)
        try:
            return slot.get(timeout=CALL_LIMIT_S)
        except queue.Empty:
            with self.lock:
                pending = self.pending.get(id)
            if pending is not None:
                pending[0].terminate()
            log.warning("loom hook %s: no answer in %s s", ENTRY, CALL_LIMIT_S)
            return None

    def close(self):
        with self.lock:
            child, self.child = self.child, None
        if child is not None and child.stdin is not None:
            try:
                child.stdin.close()
            except OSError:
                pass


class Loom:
    def __init__(self, ctx):
        self.ctx = ctx
        self.hook = Hook()
        self.lock = threading.Lock()
        self.session = None
        self.turn = None
        self.model = None
        self.effort = os.environ.pop(EFFORT_ENV, None) or None
        self.children = set()
        self.ours = []
        self.reply = None
        self.watching = None

    def state(self):
        return {"transcript": self.session, "model": self.model, "effort": self.effort}

    def send(self, event, **fields):
        return self.hook.send({"event": event, **fields, **self.state()})

    def call(self, event, **fields):
        return self.hook.call({"event": event, **fields, **self.state()})

    def handed(self, reply):
        if not reply:
            return None
        if reply.get("ids"):
            self.send("ack", ids=reply["ids"])
        return reply.get("text")

    def main(self, session_id):
        return bool(session_id) and session_id not in self.children

    def follow(self, session_id, model):
        self.session = session_id
        self.model = model or self.model

    def claim(self, message):
        with self.lock:
            for at, (text, ids) in enumerate(self.ours):
                if text in message:
                    del self.ours[at]
                    return ids
        return None

    def inject(self, text, ids=None):
        if ids is not None:
            with self.lock:
                self.ours.append((text, ids))
        for _ in range(INJECT_TRIES):
            if self.ctx.inject_message(text):
                return
            time.sleep(INJECT_PAUSE_S)
        log.warning("loom: Hermes did not take a loom message")
        if ids is not None:
            with self.lock:
                if (text, ids) in self.ours:
                    self.ours.remove((text, ids))

    def wait(self, event, **fields):
        slot = self.send(event, **fields)[1]
        threading.Thread(target=self.waited, args=(slot,), daemon=True).start()

    def waited(self, slot):
        reply = slot.get()
        if reply and reply.get("text"):
            self.inject(reply["text"], reply.get("ids") or [])

    def watch(self):
        with self.lock:
            if self.watching is not None:
                return
            id, slot = self.send("interrupt")
            self.watching = id
        threading.Thread(target=self.watched, args=(id, slot), daemon=True).start()

    def watched(self, id, slot):
        reply = slot.get()
        with self.lock:
            if self.watching != id:
                return
            self.watching = None
        if reply and reply.get("interrupted"):
            self.inject(f"/{STOP}")

    def unwatch(self):
        with self.lock:
            id, self.watching = self.watching, None
        if id is not None:
            self.send("close", of=id)

    def on_session_start(self, session_id=None, model=None, **_):
        if not self.main(session_id):
            return
        self.follow(session_id, model)
        self.call("session_start")

    def pre_llm_call(self, session_id=None, turn_id=None, user_message=None, model=None, **_):
        if not self.main(session_id):
            return None
        self.follow(session_id, model)
        self.turn = turn_id
        message = user_message if isinstance(user_message, str) else json.dumps(user_message)
        ids = self.claim(message)
        if ids is not None:
            if ids:
                self.send("ack", ids=ids)
            self.watch()
            return None
        text = self.handed(self.call("prompt", prompt=message))
        self.watch()
        return {"context": text} if text else None

    def in_turn(self, session_id, turn_id):
        return bool(self.turn) and session_id == self.session and turn_id == self.turn

    def transform_tool_result(self, result=None, session_id=None, turn_id=None, **_):
        if not self.in_turn(session_id, turn_id):
            return None
        text = self.handed(self.call("tool_result"))
        return f"{result}\n\n{text}" if text else None

    def post_llm_call(self, session_id=None, turn_id=None, assistant_response=None, **_):
        if self.in_turn(session_id, turn_id):
            self.reply = assistant_response

    def on_session_end(self, session_id=None, turn_id=None, interrupted=False, **_):
        if not self.in_turn(session_id, turn_id):
            return
        self.turn = None
        self.unwatch()
        reply, self.reply = self.reply, None
        with self.lock:
            if self.ours:
                return
        if interrupted:
            self.wait("idle")
        else:
            self.wait("settled", reply=reply)

    def subagent_start(self, child_session_id=None, **_):
        if child_session_id:
            self.children.add(child_session_id)


def register(ctx):
    loom = Loom(ctx)
    for name in (
        "on_session_start",
        "pre_llm_call",
        "transform_tool_result",
        "post_llm_call",
        "on_session_end",
        "subagent_start",
    ):
        ctx.register_hook(name, getattr(loom, name))
    ctx.register_command(STOP, lambda _args: None, description="loom: end of an interrupted turn")
    for skill in sorted(SKILLS.glob("*/SKILL.md")):
        ctx.register_skill(skill.parent.name, skill)
    if os.environ.pop(IDLE_ENV, None) == "1":
        loom.wait("idle")
