"""Check the smoke harness reader before using it to inspect real PTY output."""

import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("smoke", Path(__file__).parents[1] / "scripts" / "smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class ScreenTests(unittest.TestCase):
    def test_split_escape_utf8_and_hyperlink(self):
        screen = smoke.Screen(20, 4)
        for byte in "\x1b[2;3H┌Otto\x1b]8;;https://example.org\x1b\\ link\x1b]8;;\x1b\\".encode():
            screen.feed(bytes([byte]))
        self.assertEqual(screen.text().splitlines()[1], "  ┌Otto link")

    def test_cursor_rewrite_and_clear(self):
        screen = smoke.Screen(20, 4)
        screen.feed(b"old text\x1b[1;1Hnew\x1b[K\x1b[3;1Hbottom")
        self.assertEqual(screen.text().splitlines(), ["new", "", "bottom"])
        screen.feed(b"\x1b[2J\x1b[1;1Hfresh")
        self.assertEqual(screen.text().strip(), "fresh")

    def test_sgr_and_private_modes_do_not_render(self):
        screen = smoke.Screen(20, 4)
        screen.feed(b"\x1b[?1049h\x1b[38;2;189;98;43mCopperlane\x1b[0m\x1b[?25l")
        self.assertEqual(screen.text().strip(), "Copperlane")

    def test_cursor_saved_around_hyperlink_overlay(self):
        screen = smoke.Screen(20, 4)
        screen.feed(b"body\x1b7\x1b[3;1Hlink\x1b8 next")
        self.assertEqual(screen.text().splitlines(), ["body next", "", "link"])


class WireTests(unittest.TestCase):
    def test_osc52_payloads_decode_bel_and_st_terminators(self):
        wire = (b"frame\x1b]52;c;aHR0cHM6Ly93d3cuY29wcGVybGFuZS5haS8=\x07"
                b"more\x1b]52;c;aHR0cHM6Ly9naXRodWIuY29tL2Ficmo3L290dG8=\x1b\\")
        self.assertEqual(smoke.osc52_payloads(wire),
                         ["https://www.copperlane.ai/", "https://github.com/abrj7/otto"])
        self.assertEqual(smoke.osc52_payloads(b"\x1b]52;c;aGk="), [])

    def test_osc8_urls_skip_the_closing_sequence(self):
        wire = b"\x1b]8;;https://seanyang.ca\x1b\\Sean Yang\x1b]8;;\x1b\\ \x1b]8;id=1;https://x.com\x07X"
        self.assertEqual(smoke.osc8_urls(wire), ["https://seanyang.ca", "https://x.com"])

    def test_goodbye_must_follow_the_alternate_screen_exit(self):
        self.assertTrue(smoke.after_alternate_screen(b"\x1b[?1049h..\x1b[?1049l\r\nbye", b"bye"))
        self.assertFalse(smoke.after_alternate_screen(b"bye\x1b[?1049l", b"bye"))
        self.assertFalse(smoke.after_alternate_screen(b"bye", b"bye"))

    def test_log_leaks_exempt_only_the_listen_banner(self):
        log = ("Portfolio SSH listening on 127.0.0.1:2222 (maximum 8 connections, 2 per address)\n"
               "session connect (active 1)\n"
               "session disconnect (duration 0s, size 80x24, reason quit)\n")
        self.assertEqual(smoke.log_leaks(log), [])
        self.assertEqual(smoke.log_leaks(log + "session connect from 127.0.0.1:50122\n"),
                         ["session connect from 127.0.0.1:50122"])
        self.assertEqual(smoke.log_leaks("auth none for visitor\n"), ["auth none for visitor"])


if __name__ == "__main__":
    unittest.main()
