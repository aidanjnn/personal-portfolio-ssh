#!/usr/bin/env python3
"""Real PTY and localhost OpenSSH checks; Python standard library only.

Run after `cargo build --locked`. Screens and wire transcripts are saved under
the chosen --output directory to make failures inspectable.
"""

from __future__ import annotations

import argparse
import base64
import codecs
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import time
import unicodedata


def terminal_environment():
    environment = dict(os.environ)
    # Automation shells often set NO_COLOR; these PTYs model a colour terminal.
    environment.pop("NO_COLOR", None)
    environment.update(TERM="xterm-256color", COLORTERM="truecolor")
    return environment


def restored_terminal(before, after):
    # BSD marks pending canonical input for reprinting when OpenSSH restores it.
    # PENDIN is transient kernel bookkeeping, unlike ICANON/ECHO/ISIG and speeds.
    expected, actual = list(before), list(after)
    pendin = getattr(termios, "PENDIN", 0)
    expected[3] &= ~pendin
    actual[3] &= ~pendin
    return actual == expected


class Screen:
    """Small VT screen reader for the cursor operations used by Crossterm.

    This is intentionally a test aid, not a general terminal emulator. Styles,
    OSC hyperlinks and private modes do not change the text cells inspected.
    """

    def __init__(self, columns=80, rows=24):
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.pending = ""
        self.x = self.y = 0
        self.saved = (0, 0)
        self.resize(columns, rows)

    def resize(self, columns, rows):
        self.columns, self.rows = columns, rows
        self.cells = [[" "] * columns for _ in range(rows)]
        self.x = min(self.x, columns - 1)
        self.y = min(self.y, rows - 1)

    def feed(self, data):
        self.pending += self.decoder.decode(data)
        while self.pending:
            if self.pending.startswith("\x1b["):
                end = next((i for i in range(2, len(self.pending))
                            if "@" <= self.pending[i] <= "~"), None)
                if end is None:
                    break
                self.csi(self.pending[2:end], self.pending[end])
                self.pending = self.pending[end + 1:]
            elif self.pending.startswith("\x1b]"):
                bell = self.pending.find("\x07", 2)
                st = self.pending.find("\x1b\\", 2)
                ends = [(bell, 1), (st, 2)]
                ends = [(pos, size) for pos, size in ends if pos >= 0]
                if not ends:
                    break
                pos, size = min(ends)
                self.pending = self.pending[pos + size:]
            elif self.pending[0] == "\x1b":
                if len(self.pending) < 2:
                    break
                if self.pending[1] == "7":
                    self.saved = self.x, self.y
                elif self.pending[1] == "8":
                    self.x, self.y = self.saved
                # Charset designation consists of ESC, (, and one character.
                size = 3 if self.pending[1] in "()*+" else 2
                if len(self.pending) < size:
                    break
                self.pending = self.pending[size:]
            else:
                char, self.pending = self.pending[0], self.pending[1:]
                if char == "\r":
                    self.x = 0
                elif char == "\n":
                    self.y = min(self.rows - 1, self.y + 1)
                elif char == "\b":
                    self.x = max(0, self.x - 1)
                elif char >= " " and not unicodedata.combining(char):
                    width = 2 if unicodedata.east_asian_width(char) in "WF" else 1
                    if self.x < self.columns:
                        self.cells[self.y][self.x] = char
                        if width == 2 and self.x + 1 < self.columns:
                            self.cells[self.y][self.x + 1] = ""
                    self.x = min(self.columns, self.x + width)

    def csi(self, raw, command):
        if raw.startswith(("?", ">", "!")):
            return
        try:
            args = [int(value or "0") for value in raw.split(";")]
        except ValueError:
            return
        n = args[0] or 1
        if command in "Hf":
            self.y = max(0, min(self.rows - 1, n - 1))
            self.x = max(0, min(self.columns - 1, (args[1] or 1) - 1 if len(args) > 1 else 0))
        elif command == "G":
            self.x = max(0, min(self.columns - 1, n - 1))
        elif command == "d":
            self.y = max(0, min(self.rows - 1, n - 1))
        elif command in "ABCD":
            self.x = max(0, min(self.columns - 1, self.x + (n if command == "C" else -n if command == "D" else 0)))
            self.y = max(0, min(self.rows - 1, self.y + (n if command == "B" else -n if command == "A" else 0)))
        elif command == "J":
            mode = args[0]
            if mode in (2, 3):
                self.cells = [[" "] * self.columns for _ in range(self.rows)]
            elif mode == 0:
                self.cells[self.y][self.x:] = [" "] * (self.columns - self.x)
                for row in range(self.y + 1, self.rows):
                    self.cells[row] = [" "] * self.columns
        elif command == "K":
            mode = args[0]
            start, end = (0, self.columns) if mode == 2 else (0, self.x + 1) if mode == 1 else (self.x, self.columns)
            self.cells[self.y][start:end] = [" "] * (end - start)

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.cells)


OSC8 = b"\x1b]8;"
OSC52 = b"\x1b]52;c;"
LEAVE_ALTERNATE = b"\x1b[?1049l"
PAPER_CANVAS = b"48;2;255;255;255"
NIGHT_CANVAS = b"48;2;17;22;27"
GOODBYE = b"thanks for stopping by"
TIME_LIMIT = b"session time limit reached"
RESTARTING = b"the portfolio is restarting"
HOME = "University of Waterloo"
COPPERLANE_URL = "https://www.copperlane.ai/"
OTTO_URL = "https://github.com/abrj7/otto"
ENGINEERING_URL = "https://uwaterloo.ca/engineering/"
COMPACT_HEADER = "Aidan Jeon  996  Projects  Thoughts"


def osc52_payloads(data):
    """Decoded clipboard writes (OSC 52, BEL or ST terminated) in a byte stream."""
    payloads, start = [], 0
    while (start := data.find(OSC52, start)) >= 0:
        body = start + len(OSC52)
        ends = [end for end in (data.find(b"\x07", body), data.find(b"\x1b\\", body)) if end >= 0]
        if not ends:
            break
        payloads.append(base64.b64decode(data[body:min(ends)], validate=True).decode())
        start = min(ends)
    return payloads


def osc8_urls(data):
    """Targets opened by OSC 8 hyperlinks; the empty closing target is skipped."""
    urls, start = [], 0
    while (start := data.find(OSC8, start)) >= 0:
        params_end = data.find(b";", start + len(OSC8))
        ends = [end for end in (data.find(b"\x07", params_end), data.find(b"\x1b\\", params_end)) if end >= 0]
        if params_end < 0 or not ends:
            break
        url = data[params_end + 1:min(ends)].decode()
        if url:
            urls.append(url)
        start = min(ends)
    return urls


def after_alternate_screen(data, text):
    """True when `text` is written after the last alternate-screen exit."""
    leave = data.rfind(LEAVE_ALTERNATE)
    return leave >= 0 and text in data[leave:]


def log_leaks(log, secrets=("127.0.0.1", "visitor")):
    """Server log lines that name a client address or user.

    The startup banner names the configured listen address, which is the
    operator's own setting rather than visitor data, so it is exempt.
    """
    return [line for line in log.splitlines()
            if not line.startswith("Portfolio SSH listening on")
            and any(secret in line for secret in secrets)]


class Terminal:
    def __init__(self, command, output, name, columns=80, rows=24):
        self.output, self.name = output, name
        self.master, self.slave = pty.openpty()
        self.before = termios.tcgetattr(self.slave)
        self.screen = Screen(columns, rows)
        self.wire = bytearray()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        self.process = subprocess.Popen(command, stdin=self.slave, stdout=self.slave,
                                        stderr=self.slave, start_new_session=True,
                                        env=terminal_environment())

    def drain(self, duration=0.2):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            ready, _, _ = select.select([self.master], [], [], max(0, until - time.monotonic()))
            if ready:
                try:
                    data = os.read(self.master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not data:
                    break
                self.wire.extend(data)
                self.screen.feed(data)

    def mark(self):
        """Wire offset for asserting on output produced after this point."""
        return len(self.wire)

    def since(self, mark):
        return bytes(self.wire[mark:])

    def fail(self, message):
        self.capture("failure")
        raise AssertionError(f"{self.name}: {message}\n{self.screen.text()}")

    def wait_for(self, *texts, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain(0.1)
            screen = self.screen.text()
            if all(text in screen for text in texts):
                return
            if self.process.poll() is not None:
                break
        missing = [text for text in texts if text not in self.screen.text()]
        self.fail(f"did not render {missing!r}")

    def wait_gone(self, text, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain(0.1)
            if text not in self.screen.text():
                return
        self.fail(f"still renders {text!r}")

    def wait_wire(self, needle, mark, timeout=8):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.drain(0.1)
            if needle in self.since(mark):
                return
        self.fail(f"did not send {needle!r}")

    def send(self, keys):
        os.write(self.master, keys)
        self.drain()

    def resize(self, columns, rows):
        self.screen.resize(columns, rows)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        os.killpg(self.process.pid, signal.SIGWINCH)
        self.drain(0.5)

    def capture(self, label):
        path = self.output / f"{self.name}-{label}.txt"
        path.write_text(self.screen.text() + "\n")
        return path

    def restored(self):
        return restored_terminal(self.before, termios.tcgetattr(self.slave))

    def quit(self, keys=b"q", expect_restored=True, expect_goodbye=True):
        self.send(keys)
        status = self.process.wait(timeout=5)
        self.drain()
        assert status == 0, f"{self.name}: exit status {status}"
        if expect_goodbye:
            assert after_alternate_screen(self.wire, GOODBYE), \
                f"{self.name}: goodbye line missing or written before leaving the alternate screen"
        if expect_restored:
            after = termios.tcgetattr(self.slave)
            assert restored_terminal(self.before, after), f"{self.name}: terminal attributes not restored\nbefore={self.before!r}\nafter={after!r}"

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        (self.output / f"{self.name}.ansi").write_bytes(self.wire)
        os.close(self.master)
        os.close(self.slave)


class Checks:
    """Named checks that passed, in order, so a run reports what it covered."""

    def __init__(self, suite):
        self.suite, self.passed, self.skipped = suite, [], []

    def ok(self, name):
        self.passed.append(name)

    def skip(self, name, reason):
        self.skipped.append(f"{name}: {reason}")


def check_home(terminal, checks, prefix=""):
    terminal.wait_for("Aidan Jeon", "996", HOME)
    checks.ok(f"{prefix}home renders name, nav and intro")


def check_local(binary, output):
    checks = Checks("local")
    terminal = Terminal([str(binary)], output, "local")
    try:
        mark = terminal.mark()
        check_home(terminal, checks)
        terminal.capture("80x24-home")
        assert not (termios.tcgetattr(terminal.slave)[3] & termios.ICANON), "local app did not enter raw mode"
        checks.ok("raw mode while running")
        assert PAPER_CANVAS in terminal.since(mark), "Paper theme did not paint the white canvas"
        checks.ok("Paper paints 48;2;255;255;255")

        terminal.send(b"G")
        terminal.wait_for("you found the terminal version")
        terminal.capture("80x24-home-bottom")
        checks.ok("home note after G")
        terminal.send(b"g")
        terminal.wait_for("Updated")
        # G scrolled the focused link off screen, which drops focus; Tab picks the first link.
        terminal.send(b"\t")

        mark = terminal.mark()
        terminal.send(b"c")
        terminal.wait_wire(OSC52, mark)
        terminal.wait_for("copied")
        payloads = osc52_payloads(terminal.since(mark))
        assert payloads == [ENGINEERING_URL], f"OSC 52 payloads {payloads!r}"
        checks.ok("c emits OSC 52 with the focused URL")

        mark = terminal.mark()
        terminal.send(b"d")
        terminal.wait_wire(NIGHT_CANVAS, mark)
        assert PAPER_CANVAS not in terminal.since(mark), "Night frame still painted Paper"
        mark = terminal.mark()
        terminal.send(b"d")
        terminal.drain(0.6)
        plain = terminal.since(mark)
        assert plain and b"38;2;" not in plain and b"48;2;" not in plain, "Plain frame used truecolor"
        terminal.wait_for(HOME)
        mark = terminal.mark()
        terminal.send(b"d")
        terminal.wait_wire(PAPER_CANVAS, mark)
        checks.ok("d cycles Paper -> Night -> Plain -> Paper on the wire")

        terminal.send(b"w")
        terminal.wait_for("Work", "Copperlane (YC W26) ▾")
        terminal.send(b"\r")
        terminal.wait_for("intelligence layer", "▴")
        terminal.capture("80x24-work")
        checks.ok("w opens Work; Enter expands Copperlane")
        mark = terminal.mark()
        terminal.send(b"o")
        terminal.wait_for(COPPERLANE_URL, "cmd/ctrl-click to open")
        terminal.wait_wire(OSC8, mark)
        assert COPPERLANE_URL in osc8_urls(terminal.since(mark)), "Copperlane sheet lacked an OSC 8 link"
        terminal.capture("80x24-work-link")
        checks.ok("o shows the copperlane.ai sheet with OSC 8")
        terminal.send(b"\x1b")
        terminal.wait_gone(COPPERLANE_URL)

        terminal.send(b"p")
        terminal.wait_for("Projects", "teachAR", "Otto")
        terminal.send(b"j")
        terminal.wait_for("↗ github.com/abrj7/otto")
        terminal.send(b"\r")
        terminal.wait_for(OTTO_URL)
        terminal.capture("80x24-projects-link")
        terminal.send(b"\x1b")
        terminal.wait_gone(OTTO_URL)
        checks.ok("p lists projects; Otto + Enter opens its sheet; Esc closes it")

        terminal.send(b"t")
        terminal.wait_for("Thoughts", "misc.", "↵ read")
        terminal.send(b"\r")
        terminal.wait_for("productivity stack", "esc back to Thoughts", "↓ ")
        terminal.capture("80x24-reader")
        terminal.send(b"G")
        terminal.wait_gone("↓ ")
        bottom = terminal.screen.text()
        terminal.send(b"j" * 20)
        terminal.drain(0.3)
        assert terminal.screen.text() == bottom, "reader scrolled past its bottom"
        terminal.capture("80x24-reader-bottom")
        terminal.send(b"\x1b")
        terminal.wait_for("↵ read")
        terminal.wait_gone("esc back to Thoughts")
        checks.ok("t + Enter opens the reader; G stops at the bottom; Esc returns")

        terminal.send(b"?")
        terminal.wait_for("Sean Yang", "seanyang.ca")
        terminal.capture("80x24-help")
        terminal.send(b"\x1b")
        terminal.wait_gone("seanyang.ca")
        checks.ok("? credits Sean Yang / seanyang.ca")

        terminal.send(b"h")
        terminal.resize(40, 16)
        terminal.wait_for(COMPACT_HEADER, HOME)
        terminal.capture("40x16-home")
        checks.ok("40x16 compact header")
        terminal.resize(24, 8)
        terminal.wait_for("Resize to at least")
        terminal.capture("24x8-resize-message")
        checks.ok("24x8 resize message")
        terminal.quit()
        checks.ok("q leaves the alternate screen, prints goodbye, restores the terminal")
    finally:
        terminal.close()

    terminal = Terminal([str(binary)], output, "local-interrupt", 40, 16)
    try:
        terminal.wait_for(COMPACT_HEADER)
        terminal.send(b"p\t")
        terminal.quit(b"\x03")
        checks.ok("Ctrl+C quits with goodbye and restores the terminal")
    finally:
        terminal.close()

    terminal = Terminal([str(binary), "--no-links"], output, "local-no-links")
    try:
        check_home(terminal, Checks("ignored"))
        terminal.send(b"pj\r")
        terminal.wait_for(OTTO_URL)
        terminal.quit()
        assert OSC8 not in terminal.wire, "--no-links still emitted OSC 8"
        checks.ok("--no-links keeps readable URLs and emits no OSC 8")
    finally:
        terminal.close()
    return checks


def run(command, **kwargs):
    return subprocess.run(command, text=True, capture_output=True, timeout=10, **kwargs)


def fingerprint(public_key_line, directory):
    path = directory / "fingerprint.pub"
    path.write_text(public_key_line + "\n")
    result = run(["ssh-keygen", "-l", "-E", "sha256", "-f", str(path)])
    assert result.returncode == 0, result.stderr
    return result.stdout.split()[1]


def address_available(address):
    try:
        with socket.socket() as probe:
            probe.bind((address, 0))
        return True
    except OSError:
        return False


def check_ssh(binary, output, port, slow=False):
    checks = Checks("ssh")
    for tool in ("ssh", "ssh-keygen", "ssh-keyscan"):
        if not shutil.which(tool):
            raise RuntimeError(f"{tool} is required for SSH smoke checks")
    # Refuse to reuse a listener that the test did not start.
    with socket.socket() as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        probe.bind(("127.0.0.1", port))
    with tempfile.TemporaryDirectory(prefix="portfolio-ssh-smoke-") as temp:
        directory = Path(temp)
        key = directory / "host_key"
        known = directory / "known_hosts"
        log_path = output / "ssh-server.log"
        log = log_path.open("wb")
        server = None
        sessions = []

        def start(max_sessions=8, idle_timeout=30, max_per_ip=2, extra=()):
            began = time.monotonic()
            process = subprocess.Popen([str(binary), "serve", "--bind", f"127.0.0.1:{port}",
                                        "--host-key", str(key), "--max-sessions", str(max_sessions),
                                        "--max-per-ip", str(max_per_ip),
                                        "--idle-timeout", str(idle_timeout), *extra],
                                       stdout=log, stderr=log, env=terminal_environment())
            deadline = began + 15
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise AssertionError("SSH server exited; see ssh-server.log")
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                        return process, time.monotonic() - began
                except OSError:
                    time.sleep(0.1)
            process.kill()
            raise AssertionError("SSH server never listened")

        def stop(process, sig=signal.SIGTERM):
            """Stop the server the way docker or systemd does; it must exit cleanly."""
            process.send_signal(sig)
            status = process.wait(timeout=10)
            assert status == 0, f"server exited with status {status} on {sig.name}"

        def server_log():
            return log_path.read_text(errors="replace")

        def wait_idle(timeout=8):
            """Wait until every logged connection has logged its disconnect."""
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                text = server_log()
                if text.count("session connect") == text.count("session disconnect"):
                    return
                time.sleep(0.1)
            raise AssertionError("server kept connection slots after clients left; see ssh-server.log")

        def session(name, columns=80, rows=24, extra=()):
            terminal = Terminal(options + list(extra) + ["-tt", destination], output, name, columns, rows)
            sessions.append(terminal)
            return terminal

        def assert_refused(terminal, why):
            status = terminal.process.wait(timeout=8)
            terminal.drain()
            terminal.capture("refused")
            assert status != 0, f"{why}: visitor was not refused"
            assert HOME not in terminal.screen.text(), f"{why}: refused visitor saw the portfolio"

        options = ["ssh", "-F", "/dev/null", "-p", str(port), "-o", "BatchMode=yes",
                   "-o", "StrictHostKeyChecking=yes", "-o", f"UserKnownHostsFile={known}",
                   "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ConnectTimeout=5",
                   "-o", "PreferredAuthentications=none", "-o", "LogLevel=ERROR"]
        destination = "visitor@127.0.0.1"
        try:
            server, _ = start()
            public = run(["ssh-keygen", "-y", "-f", str(key)])
            assert public.returncode == 0, public.stderr
            fields = public.stdout.split()
            assert len(fields) >= 2
            # Trust the exact key created by our own server, not an unchecked scan.
            known.write_text(f"[127.0.0.1]:{port} {fields[0]} {fields[1]}\n")
            original_key = key.read_bytes()
            assert key.stat().st_mode & 0o777 == 0o600, "host key permissions must be 0600"
            checks.ok("host key created with mode 0600")
            scan = run(["ssh-keyscan", "-T", "3", "-p", str(port), "-t", "ed25519", "127.0.0.1"])
            scanned = next((line.split()[1:3] for line in scan.stdout.splitlines()
                            if "ssh-ed25519" in line), None)
            assert scanned and scanned[1] == fields[1], "served SSH key differs from persisted host key"
            persisted_fp = fingerprint(f"{fields[0]} {fields[1]}", directory)
            assert fingerprint(" ".join(scanned), directory) == persisted_fp, "fingerprints differ"
            checks.ok(f"served key matches the persisted key ({persisted_fp})")

            first = session("ssh-first")
            check_home(first, checks, "SSH ")
            second = session("ssh-second", 40, 16)
            second.wait_for(COMPACT_HEADER, HOME)
            second_initial = second.screen.text()

            # The cap is per address: two localhost visitors are active now.
            refused = session("ssh-per-ip-refused")
            assert_refused(refused, "--max-per-ip 2")
            checks.ok("third concurrent localhost visitor refused by --max-per-ip 2")
            if address_available("127.0.0.2"):
                other = session("ssh-per-ip-other-address", extra=["-b", "127.0.0.2"])
                other.wait_for(HOME)
                other.quit()
                checks.ok("a visitor from another address still enters while localhost is capped")
            else:
                checks.skip("per-address isolation", "127.0.0.2 is not a local address on this host")

            first.send(b"w")
            first.wait_for("Copperlane (YC W26) ▾")
            first.send(b"\r")
            first.wait_for("intelligence layer")
            mark = first.mark()
            first.send(b"o")
            first.wait_for(COPPERLANE_URL)
            first.wait_wire(OSC8, mark)
            assert COPPERLANE_URL in osc8_urls(first.since(mark)), "SSH sheet lacked an OSC 8 link"
            first.capture("80x24-work-link")
            checks.ok("SSH Work expansion and copperlane.ai sheet with OSC 8")
            mark = first.mark()
            first.send(b"c")
            first.wait_wire(OSC52, mark)
            assert osc52_payloads(first.since(mark)) == [COPPERLANE_URL], "SSH OSC 52 payload"
            checks.ok("SSH c emits OSC 52 with the focused URL")
            second.drain()
            assert second.screen.text() == second_initial, "navigation leaked between SSH sessions"
            second.capture("40x16-home")
            checks.ok("two SSH sessions navigate independently")

            first.send(b"\x1b")
            first.wait_gone(COPPERLANE_URL)
            first.resize(40, 16)
            first.wait_for(COMPACT_HEADER, "Copperlane (YC W26) ▴")
            first.capture("40x16-resized")
            checks.ok("SSH window-change reflows to the compact layout")
            # An arrow key split across writes is one event, not Escape + stray text.
            os.write(first.master, b"\x1b[")
            time.sleep(0.15)
            first.send(b"B")
            first.wait_for("uwblueprint.org")
            checks.ok("CSI arrow split by 150ms decodes as one key")
            first.send(b"?")
            first.wait_for("Sean Yang", "seanyang.ca")
            first.capture("40x16-help")
            first.send(b"\x1b")
            first.wait_gone("seanyang.ca")
            checks.ok("SSH help sheet credits Sean Yang")
            first.quit()
            checks.ok("SSH q: exit 0, goodbye after leaving the alternate screen, terminal restored")
            second.process.kill()
            second.process.wait(timeout=5)
            wait_idle()
            checks.ok("abrupt client death releases its slot")

            for index in range(3):
                repeated = session(f"ssh-repeat-{index}")
                repeated.wait_for(HOME)
                if index == 0:
                    repeated.resize(40, 16)
                    repeated.wait_for(COMPACT_HEADER)
                    # Page jump + focus + End in one packet must use the new page's bounds.
                    repeated.send(b"p\t\x1b[F")
                    # The compact status bar truncates the URL to "github.com/abrj7/…".
                    repeated.wait_for("↗ github.com/abrj7/", "Sketchy", "© Aidan Jeon 2026")
                    repeated.capture("40x16-batched-scroll")
                # Page jump + focus + Enter in one packet must open the new focus's link.
                repeated.send(b"pj\r")
                repeated.wait_for(OTTO_URL)
                repeated.quit(b"\x03" if index == 1 else b"q")
                wait_idle()
            checks.ok("repeat connects; batched p/Tab/End and p/j/Enter; q and Ctrl+C")

            for label, args in (
                ("exec", ["-T", destination, "printf forbidden-command"]),
                ("sftp", ["-s", destination, "sftp"]),
                ("no-pty", ["-T", destination]),
                ("remote-forward", ["-N", "-o", "ExitOnForwardFailure=yes", "-R", "0:127.0.0.1:1", destination]),
                ("direct-forward", ["-W", "127.0.0.1:1", destination]),
            ):
                result = run(options + args, input="")
                (output / f"ssh-reject-{label}.txt").write_text(result.stdout + result.stderr)
                assert result.returncode != 0, f"SSH accepted unsupported {label} request"
                assert "forbidden-command" not in result.stdout, "server executed a shell command"
                wait_idle()
            checks.ok("exec, sftp subsystem, no-PTY shell, remote and direct forwarding rejected")

            for sig in (signal.SIGTERM, signal.SIGINT):
                live = session(f"ssh-live-{sig.name.lower()}")
                live.wait_for(HOME)
                started = time.monotonic()
                server.send_signal(sig)
                # Keep reading the visitor's PTY: a full PTY would block ssh, not the server.
                deadline = started + 10
                while time.monotonic() < deadline and None in (server.poll(), live.process.poll()):
                    live.drain(0.1)
                live.drain()
                live.capture("stopped")
                assert server.poll() == 0, f"{sig.name}: server exited with {server.poll()}"
                server = None
                status = live.process.poll()
                assert after_alternate_screen(live.wire, RESTARTING), \
                    f"{sig.name}: restart notice missing or written before leaving the alternate screen"
                assert live.restored(), f"{sig.name}: visitor's terminal attributes not restored"
                assert status == 0, f"{sig.name}: visitor's ssh exited with status {status}"
                checks.ok(f"{sig.name} with a live visitor: screen restored, restart notice, "
                          f"server exits 0 in {time.monotonic() - started:.1f}s")
                server, _ = start()
            assert key.read_bytes() == original_key, "host key changed across restart"
            restarted = session("ssh-restarted")
            check_home(restarted, Checks("ignored"))
            restarted.quit()
            wait_idle()
            checks.ok("restart keeps the host key and serves the bundled page")

            stop(server)
            server, _ = start(max_sessions=1, idle_timeout=30)
            limited = session("ssh-limit-active")
            limited.wait_for(HOME)
            refused = session("ssh-limit-refused")
            assert_refused(refused, "--max-sessions 1")
            limited.process.kill()
            limited.process.wait(timeout=5)
            wait_idle()
            recovered = session("ssh-limit-recovered")
            recovered.wait_for(HOME)
            recovered.quit()
            wait_idle()
            checks.ok("--max-sessions 1 refuses a second visitor, then frees the slot")

            if slow:
                stop(server)
                server, _ = start(idle_timeout=300, extra=["--max-session-secs", "60"])
                limited = session("ssh-session-limit")
                limited.wait_for(HOME)
                deadline = time.monotonic() + 80
                while limited.process.poll() is None and time.monotonic() < deadline:
                    limited.drain(0.5)
                limited.drain()
                limited.capture("time-limit")
                assert limited.process.poll() is not None, "session outlived --max-session-secs 60"
                assert after_alternate_screen(limited.wire, TIME_LIMIT), "time-limit message missing"
                assert limited.restored(), "time limit left terminal attributes changed"
                wait_idle()
                assert "reason limit" in server_log(), "time-limit disconnect not logged with its reason"
                checks.ok("--max-session-secs 60 ends the session with 'session time limit reached'")
            else:
                checks.skip("--max-session-secs 60", "takes about a minute; run with --slow")

            stop(server)
            server, _ = start(idle_timeout=1)
            idle = session("ssh-idle")
            idle.wait_for(HOME)
            deadline = time.monotonic() + 5
            while idle.process.poll() is None and time.monotonic() < deadline:
                idle.drain(0.1)
            idle.capture("timeout-result")
            assert idle.process.poll() is not None, "idle connection remained open beyond its configured timeout"
            idle.drain()
            assert idle.restored(), "idle disconnect left terminal attributes changed"
            assert LEAVE_ALTERNATE in idle.wire, "idle timeout did not leave alternate screen"
            wait_idle()
            assert "reason idle" in server_log(), "idle disconnect not logged with its reason"
            checks.ok("idle timeout restores the terminal and logs reason idle")

            text = server_log()
            assert "session connect" in text and "session disconnect" in text, "session lifecycle not logged"
            leaks = log_leaks(text)
            assert not leaks, f"server log names a client address or user: {leaks!r}"
            checks.ok("log has session connect/disconnect and no client address or username")
        finally:
            for terminal in sessions:
                terminal.close()
            if server is not None and server.poll() is None:
                # No assertions here: they would hide the failure that got us here.
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
            log.close()
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/personal-portfolio-tui"))
    parser.add_argument("--output", type=Path, default=Path("target/smoke"))
    parser.add_argument("--ssh", action="store_true", help="also exercise a real localhost SSH server")
    parser.add_argument("--ssh-only", action="store_true")
    parser.add_argument("--slow", action="store_true",
                        help="also wait out a 60-second --max-session-secs limit over SSH")
    parser.add_argument("--port", type=int, default=2222)
    args = parser.parse_args()
    binary, output = args.binary.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    assert binary.is_file(), f"Build first: cargo build --locked ({binary} missing)"
    suites = []
    try:
        if not args.ssh_only:
            suites.append(check_local(binary, output))
        if args.ssh or args.ssh_only:
            suites.append(check_ssh(binary, output, args.port, args.slow))
    except Exception:
        print(f"FAIL: inspect {output}")
        raise
    results = {suite.suite: {"passed": suite.passed, "skipped": suite.skipped} for suite in suites}
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    for suite in suites:
        for name in suite.passed:
            print(f"PASS {suite.suite}: {name}")
        for note in suite.skipped:
            print(f"SKIP {suite.suite}: {note}")
        print(f"{suite.suite}: {len(suite.passed)} passed, {len(suite.skipped)} skipped")
    print(f"Evidence: {output}")


if __name__ == "__main__":
    main()
