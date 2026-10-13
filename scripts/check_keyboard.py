"""Boot a production image, type through QEMU, and verify VGA output and idle HLT."""

import argparse
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


class Monitor:
    def __init__(self, process):
        self.process = process
        self.messages = queue.Queue()
        self.sequence = 0
        threading.Thread(target=self.read_messages, daemon=True).start()
        if "QMP" not in self.receive():
            raise RuntimeError("QEMU did not send a QMP greeting")
        self.command("qmp_capabilities")

    def read_messages(self):
        try:
            for line in self.process.stdout:
                self.messages.put(json.loads(line))
        finally:
            self.messages.put(None)

    def receive(self):
        message = self.messages.get(timeout=10)
        if message is None:
            raise RuntimeError("QEMU closed its monitor")
        return message

    def command(self, name, **arguments):
        self.sequence += 1
        request = {"execute": name, "arguments": arguments, "id": self.sequence}
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        while True:
            response = self.receive()
            if response.get("id") == self.sequence:
                if "error" in response:
                    raise RuntimeError(response["error"])
                return response["return"]

    def human(self, command):
        return self.command("human-monitor-command", **{"command-line": command})


def wait_for(check, description):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(f"Timed out: {description}")


def verify(image, qemu):
    with tempfile.TemporaryDirectory(prefix="baddcafe-keyboard-") as directory:
        directory = Path(directory)
        serial = directory / "serial.txt"
        screen = directory / "vga.bin"
        with (directory / "qemu.log").open("w+") as errors:
            process = subprocess.Popen(
                [
                    qemu,
                    "-drive", f"format=raw,file={image.resolve().as_posix().replace(',', ',,')}",
                    "-smp", "1",
                    "-display", "none",
                    "-serial", f"file:{serial.as_posix()}",
                    "-qmp", "stdio",
                    "-monitor", "none",
                    "-no-reboot",
                ],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=errors,
                text=True,
                bufsize=1,
            )
            try:
                monitor = Monitor(process)
                wait_for(
                    lambda: serial.exists() and "Async executor started" in serial.read_text(),
                    "the production executor to start",
                )

                def rows():
                    response = monitor.human(f'pmemsave 0xb8000 4000 "{screen.as_posix()}"')
                    if response.strip():
                        raise RuntimeError(response)
                    data = screen.read_bytes()
                    if len(data) != 4000:
                        raise AssertionError("Incomplete VGA memory dump")
                    return [
                        data[start:start + 160:2].decode("ascii", errors="replace").rstrip()
                        for start in range(0, 4000, 160)
                    ]

                def send(key):
                    response = monitor.human(f"sendkey {key} 30")
                    if response.strip():
                        raise RuntimeError(response)
                    time.sleep(0.1)

                # Leave only the keyboard IRQ unmasked: timer ticks must not hide
                # a lost keyboard wakeup by periodically rescuing the executor.
                response = monitor.human("o /b 0x21 0xfd")
                if response.strip():
                    raise RuntimeError(response)
                wait_for(lambda: "HLT=1" in monitor.human("info registers"), "idle HLT")
                for key in ("a", "shift-b", "shift-1", "backspace", "c"):
                    send(key)
                wait_for(lambda: rows()[-1] == "aBc", "shift, release and backspace decoding")
                send("ret")
                for _ in range(24):
                    send("x")
                wait_for(
                    lambda: rows()[-2:] == ["aBc", "x" * 24],
                    "repeated keyboard interrupts and newline output",
                )
                wait_for(lambda: "HLT=1" in monitor.human("info registers"), "return to idle HLT")
                monitor.command("quit")
                if process.wait(timeout=5) != 0:
                    raise RuntimeError("QEMU exited unsuccessfully")
                print("keyboard: shift, release, backspace, newline, 24 repeated keys [ok]")
                print("executor: idle HLT and keyboard wakeups with timer masked [ok]")
            except Exception:
                errors.flush()
                errors.seek(0)
                print(errors.read())
                if serial.exists():
                    print(serial.read_text())
                raise
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
                process.stdin.close()
                process.stdout.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path, help="production bootimage .bin file")
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    args = parser.parse_args()
    verify(args.image, args.qemu)
