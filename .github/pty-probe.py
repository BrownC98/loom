# Throwaway probe: can a second process start on the same PTY slave after the first exits?
import fcntl, os, subprocess, termios, sys

def run(slave, ctty, label):
    def pre():
        os.setsid()
        if ctty:
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    try:
        p = subprocess.Popen(["sh", "-c", f"echo {label}; exit 4"], stdin=slave, stdout=slave, stderr=slave, preexec_fn=pre)
        print(label, "exit", p.wait(timeout=5), flush=True)
    except Exception as e:
        print(label, "FAILED", repr(e), flush=True)

for first_ctty in (True, False):
    m, s = os.openpty()
    print("--- first ctty", first_ctty, "tty", os.ttyname(s))
    os.set_blocking(m, False)
    run(s, first_ctty, "first")
    run(s, True, "second-same-fd")
    try:
        s2 = os.open(os.ttyname(s), os.O_RDWR)
        run(s2, True, "second-reopened")
    except Exception as e:
        print("reopen FAILED", repr(e))
    try:
        print("master read", os.read(m, 4096))
    except Exception as e:
        print("master read FAILED", repr(e))
