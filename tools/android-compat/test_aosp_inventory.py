import unittest

from aosp_inventory import parse_rc


class InitInventoryTest(unittest.TestCase):
    def test_retains_actions_imports_overrides_and_continuations(self):
        sections = parse_rc('''# Inventory only; no services are started.
import /vendor/etc/init/hw/init.${ro.hardware}.rc
service example /system/bin/example \\
    --name "two words"
    class main
    disabled
    override
    onrestart restart peer
on property:sys.boot_completed=1 && property:test=ready
    setprop test.message "ready now"
''')
        self.assertEqual(len(sections), 3)
        self.assertEqual(sections[1]["line"], 3)
        self.assertEqual(sections[1]["header"],
                         ["service", "example", "/system/bin/example", "--name", "two words"])
        self.assertEqual([x["args"] for x in sections[1]["commands"]],
                         [["class", "main"], ["disabled"], ["override"], ["onrestart", "restart", "peer"]])
        self.assertEqual(sections[2]["commands"][0]["args"],
                         ["setprop", "test.message", "ready now"])

    def test_apostrophe_is_not_an_init_quote(self):
        section = parse_rc("on boot\n    write /test don't\n")[0]
        self.assertEqual(section["commands"][0]["args"], ["write", "/test", "don't"])

    def test_multiline_values_hashes_and_init_escapes(self):
        section = parse_rc('on boot\n    write /tags "# content owned by logd\n"\n'
                           '    setprop x a#b # trailing comment\n'
                           '    write /test a\\tb\n')[0]
        self.assertEqual(section["commands"][0]["args"],
                         ["write", "/tags", "# content owned by logd\n"])
        self.assertEqual(section["commands"][1]["line"], 4)
        self.assertEqual(section["commands"][1]["args"], ["setprop", "x", "a#b"])
        self.assertEqual(section["commands"][2]["args"], ["write", "/test", "a\tb"])

    def test_does_not_silently_discard_unparsed_commands(self):
        for text in ["service incomplete", "start orphan", "import /test\nstart orphan",
                     'on boot\nwrite /x "unterminated', "on boot\\"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse_rc(text)


if __name__ == "__main__":
    unittest.main()
