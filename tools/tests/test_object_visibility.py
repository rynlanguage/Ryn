"""Port of tests/object_visibility.rs: private fields are invisible outside their module, pub methods work."""

import unittest

from ryn_harness import TempDir, run_ryn


def write_project(directory, manifest, main, library):
    (directory / "src" / "game").mkdir(parents=True)
    (directory / "ryn.yaml").write_text(manifest, encoding="utf-8")
    (directory / "src" / "main.ryn").write_text(main, encoding="utf-8")
    (directory / "src" / "game" / "player.ryn").write_text(library, encoding="utf-8")


def main_with_private_field_read():
    return (
        "use game::player\n"
        "\n"
        "fun main() {\n"
        '    hero := player::Player { name: String("hero"), health: 50 }\n'
        "    echo hero.health\n"
        "}"
    )


class ObjectVisibilityTests(unittest.TestCase):
    def test_private_fields_are_invisible_outside_their_module_but_methods_work(self):
        with TempDir("object-visibility") as project:
            write_project(
                project,
                "name: visibility-demo\nversion: 0.1.0\nowner: guest\n",
                # main.ryn: another module; `health` is private, `health_value()` is pub.
                "".join(
                    [
                        "use game::player\n",
                        "\n",
                        "fun main() {\n",
                        '    mut hero := player::Player { name: String("hero"), health: 50 }\n',
                        "    echo hero.health_value()\n",
                        "    hero.damage(10)\n",
                        "    echo hero.health_value()\n",
                        "}",
                    ]
                ),
                # game/player.ryn
                "".join(
                    [
                        "pub struct Player {\n",
                        "    pub name: String,\n",
                        "    health: i32,\n",
                        "}\n",
                        "\n",
                        "extend Player {\n",
                        "    pub fun health_value(self) -> i32 => self.health\n",
                        "\n",
                        "    pub fun damage(mut self, amount: i32) {\n",
                        "        self.health -= amount\n",
                        "    }\n",
                        "}\n",
                    ]
                ),
            )

            # The pub method works across modules.
            output = run_ryn("run", project)
            self.assertEqual(output.code, 0, f"project failed: {output.stderr}")
            self.assertEqual(output.stdout, "50\n40\n")

            # Reading the private field directly is rejected.
            (project / "src" / "main.ryn").write_text(
                main_with_private_field_read(), encoding="utf-8"
            )
            check = run_ryn("check", project)
            self.assertNotEqual(check.code, 0, "private field access must fail")
            self.assertTrue(
                "R0426" in check.stderr and "is private" in check.stderr,
                f"expected a private-field diagnostic, got: {check.stderr}",
            )


if __name__ == "__main__":
    unittest.main()
