import importlib.metadata
import json
import os
from pathlib import Path
import queue
import re
import shlex
import subprocess
import sys
import threading
import time
import uuid


FILE = Path(__file__).resolve()
CALL_TIMEOUT = 15
STOP_TIMEOUT = 5
HOOK_PROTOCOL = 4
EVENTS_BUFFERED = 256
POLL_SECONDS = .5
MAIL_POLL_SECONDS = .5


def execute(argv, env):
    if os.name == "nt":
        raise SystemExit(subprocess.call(argv, env=env))
    os.execvpe(argv[0], argv, env)


def launch(cli, args):
    path = Path(cli).resolve()
    match = re.search(rb"#!([^\r\n]+)", path.read_bytes())
    if match:
        interpreter = match[1].decode().strip()
        command = [interpreter] if Path(interpreter).is_file() else shlex.split(interpreter)
    else:
        importlib.metadata.distribution("mistral-vibe")
        command = [sys.executable]
    execute([*command, str(FILE), "tui", *args], os.environ.copy())


def tui(args):
    from vibe.cli.launcher import _BUNDLED_BIN

    expected = os.environ["LOOM_VIBE_VERSION"]
    installed = importlib.metadata.version("mistral-vibe")
    if installed != expected or not _BUNDLED_BIN.is_file():
        raise RuntimeError(f"Mistral Vibe {expected}의 기본 터미널 실행 파일이 필요합니다. 설치판: {installed}")
    env = os.environ.copy()
    env["VIBE_APP_SERVER_CMD"] = shlex.join([sys.executable, str(FILE), "server"])
    env.pop("VIBE_APP_SERVER_BIN", None)
    execute([str(_BUNDLED_BIN), *args], env)


def send(stream, message):
    stream.write(json.dumps(message, ensure_ascii=False) + "\n")
    stream.flush()


def stop(child):
    if child.poll() is None:
        child.terminate()
    try:
        child.wait(timeout=STOP_TIMEOUT)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait(timeout=STOP_TIMEOUT)


class Bridge:
    def __init__(self, server, hook):
        self.server = server
        self.hook = hook
        self.events = queue.Queue(maxsize=EVENTS_BUFFERED)
        self.calls = {}
        self.hooks = {}
        self.opening = set()
        self.projection = None
        self.runtime = None
        self.turn = None
        self.polling = False
        self.interrupting = None
        self.next_poll = 0
        self.waiting = None
        self.unacked_deliveries = 0
        self.next = 0
        self.prefix = "loom-" + uuid.uuid4().hex + "-"
        self.seen_inputs = set()

    def read(self, source, stream):
        try:
            for raw in stream:
                self.events.put((source, json.loads(raw)))
        finally:
            self.events.put((source, None))

    def call(self, method, params, done):
        self.next += 1
        ident = self.prefix + str(self.next)
        self.calls[ident] = (time.monotonic() + CALL_TIMEOUT, done)
        send(self.server.stdin, {"jsonrpc": "2.0", "id": ident, "method": method, "params": params})

    def state(self):
        session = self.projection.state.session
        if self.runtime is None:
            return {"transcript": session.id}
        model = self.runtime.config.active_model
        return {"transcript": session.id, "model": model.alias, "effort": model.thinking}

    def request(self, event, done, **values):
        self.next += 1
        ident = self.next
        limit = None if event in ("idle", "settled") else time.monotonic() + CALL_TIMEOUT
        self.hooks[ident] = (limit, done)
        send(self.hook.stdin, {"protocol": HOOK_PROTOCOL, "id": ident, "event": event, **self.state(), **values})
        return ident

    def acknowledge(self, reply):
        def acked(_):
            self.unacked_deliveries -= 1

        if reply.get("ids"):
            self.request("ack", acked, ids=reply["ids"])
        else:
            acked(None)

    def deliver(self, reply):
        if not reply.get("text"):
            return
        params = {
            "sessionId": self.projection.state.session.id,
            "message": [{"type": "text", "text": reply["text"]}],
            "idempotencyKey": self.prefix + "mail-" + (
                "-".join(map(str, reply["ids"])) if reply.get("ids") else uuid.uuid4().hex
            ),
        }
        method = "turn/start"
        if self.turn:
            method = "turn/steer"
            params["expectedTurnId"] = self.turn
        self.waiting = None
        self.unacked_deliveries += 1
        self.call(method, params, lambda _: self.acknowledge(reply))

    def wait(self, event="idle", **values):
        self.waiting = (event, values)

    def polled(self, reply=None):
        if reply:
            self.deliver(reply)
        self.polling = False
        self.next_poll = time.monotonic() + MAIL_POLL_SECONDS

    def interrupt_turn(self, turn, reply):
        if self.turn != turn:
            self.polled()
        elif reply.get("interrupted"):
            self.interrupting = turn
            self.call(
                "turn/interrupt",
                {"sessionId": self.projection.state.session.id, "expectedTurnId": turn},
                lambda _: None,
            )
            self.polled()
        else:
            self.request("in_turn", self.polled)

    def opened(self, msg, runtime):
        from vibe.app_server.protocol import RuntimeSnapshot

        self.runtime = RuntimeSnapshot.model_validate(runtime["runtime"])
        state = self.projection.state
        state.event_id = self.projection.last_event_id
        msg["result"]["state"] = state.model_dump(mode="json", by_alias=True)
        self.request("session_start", lambda _: None)
        if os.environ.get("LOOM_VIBE_IDLE") == "1":
            self.wait()
        send(sys.stdout, msg)

    def client(self, msg):
        method = msg.get("method")
        if method in ("session/start", "session/resume", "session/continue"):
            params = msg.setdefault("params", {})
            config = params.setdefault("agentConfig", {})
            config["instructions"] = config.get("instructions", "") + "\n" + os.environ["LOOM_VIBE_USAGE"]
            skills = Path(os.environ["LOOM_VIBE_SKILLS"])
            catalog = "\n".join(str(path) for path in sorted(skills.glob("*/SKILL.md")))
            config["instructions"] += "\nAvailable loom skills. Read a relevant SKILL.md before use:\n" + catalog
            if method == "session/start":
                params["sessionOnly"] = True
                if os.environ.get("LOOM_VIBE_MODEL"):
                    params["model"] = os.environ["LOOM_VIBE_MODEL"]
                if os.environ.get("LOOM_VIBE_EFFORT"):
                    params["reasoningEffort"] = os.environ["LOOM_VIBE_EFFORT"]
            self.opening.add(msg["id"])
        send(self.server.stdin, msg)

    def notification(self, msg):
        from vibe.app_server.protocol import Notification

        params = msg.get("params", {})
        if not self.projection or params.get("sessionId") != self.projection.state.session.id:
            return
        if "eventId" in params:
            if self.projection.consume(Notification.model_validate(msg)) is None:
                return
        method = msg["method"]
        if method == "runtime/updated":
            from vibe.app_server.protocol import RuntimeSnapshot

            self.runtime = RuntimeSnapshot.model_validate(params["runtime"])
        elif method == "turn/queueUpdated":
            self.seen_inputs.intersection_update(item["id"] for item in params["queue"]["items"])
            for item in params["queue"]["items"]:
                if item["id"] not in self.seen_inputs:
                    self.seen_inputs.add(item["id"])
                    prompt = "\n".join(
                        block.get("text", "") for entry in item["entries"] for block in entry["content"]
                        if block.get("type") == "text"
                    )
                    self.request("prompt", self.deliver, prompt=prompt)
        elif method == "turn/started":
            self.turn = params["turn"]["id"]
            self.interrupting = None
            self.next_poll = 0
            self.waiting = None
        elif method == "turn/completed":
            self.interrupting = None
            self.turn = None
            if params["turn"]["status"] == "completed":
                reply = next((entry.text for entry in reversed(self.projection.history)
                              if getattr(entry, "role", None) == "assistant"
                              and entry.turn_id == params["turn"]["id"] and entry.text.strip()), None)
                self.wait("settled", reply=reply)
            else:
                self.wait()

    def response(self, msg):
        ident = msg.get("id")
        pending = self.calls.pop(ident, None)
        if pending:
            if "error" in msg:
                raise RuntimeError(f"Vibe 응답 오류: {msg['error']}")
            pending[1](msg.get("result", {}))
        elif ident in self.opening:
            from vibe.app_server.events import ClientProjection
            from vibe.app_server.models import PublicSessionState

            self.opening.remove(ident)
            if "error" in msg:
                send(sys.stdout, msg)
                return
            session = msg["result"]["state"]["session"]["id"]
            self.projection = ClientProjection(PublicSessionState.model_validate(msg["result"]["state"]))
            model = os.environ.get("LOOM_VIBE_MODEL")
            effort = os.environ.get("LOOM_VIBE_EFFORT")
            if model or effort:
                params = {"sessionId": session, "sessionOnly": True}
                if model:
                    params["modelAlias"] = model
                if effort:
                    params["reasoningEffort"] = effort
                self.call("config/model/write", params, lambda runtime: self.opened(msg, runtime))
            else:
                self.call("runtime/read", {"sessionId": session}, lambda runtime: self.opened(msg, runtime))
        else:
            send(sys.stdout, msg)

    def run(self):
        for source, stream in (("client", sys.stdin), ("server", self.server.stdout), ("hook", self.hook.stdout)):
            threading.Thread(target=self.read, args=(source, stream), daemon=True).start()
        while True:
            now = time.monotonic()
            if not self.polling and not self.unacked_deliveries:
                if self.waiting is not None and self.turn is None:
                    event, values = self.waiting
                    self.waiting = None
                    self.request(event, self.deliver, **values)
                if self.turn and self.interrupting != self.turn and now >= self.next_poll:
                    turn = self.turn
                    self.polling = True
                    self.request("interrupt_pending", lambda reply, turn=turn: self.interrupt_turn(turn, reply))
            if any(limit is not None and limit <= now for limit, _ in (*self.calls.values(), *self.hooks.values())):
                raise TimeoutError("Vibe 연결의 응답 시간이 지났습니다")
            try:
                source, msg = self.events.get(timeout=POLL_SECONDS)
            except queue.Empty:
                continue
            if msg is None:
                if source == "client":
                    return
                raise RuntimeError(f"Vibe 연결이 끊어졌습니다: {source}")
            if source == "client":
                self.client(msg)
            elif source == "hook":
                pending = self.hooks.pop(msg.get("id"), None)
                if msg.get("error"):
                    raise RuntimeError(msg["error"])
                if pending:
                    pending[1](msg)
            elif "method" in msg:
                send(sys.stdout, msg)
                self.notification(msg)
            else:
                self.response(msg)


def server():
    args = [sys.executable, "-c", "from vibe.app_server.stdio import main; main()"]
    child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    try:
        hook = subprocess.Popen(["loom", "hook", "vibe-bridge"], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, text=True, encoding="utf-8")
        try:
            Bridge(child, hook).run()
        finally:
            stop(hook)
    finally:
        stop(child)


if __name__ == "__main__":
    try:
        mode, *args = sys.argv[1:]
        if mode == "launch":
            launch(args[0], args[1:])
        elif mode == "tui":
            tui(args)
        elif mode == "server":
            server()
        else:
            raise ValueError("알 수 없는 Vibe 연결 모드입니다")
    except Exception as error:
        print(f"loom: {error}", file=sys.stderr)
        raise SystemExit(1)
