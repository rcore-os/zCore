import unittest
from apk import version


class TestApkModule(unittest.TestCase):
    def test_version_validate(self):
        self.assertTrue(version.validate("1.0"))
        self.assertFalse(version.validate("invalid-version"))

    def test_version_compare(self):
        self.assertEqual(version.compare("1.0", "1.0"), version.EQUAL)
        self.assertEqual(version.compare("1.0", "2.0"), version.LESS)
        self.assertTrue(version.compare("2.0", "1.0"), version.GREATER)

    def test_version_match(self):
        self.assertTrue(version.match("1.0", version.EQUAL, "1.0"))
        self.assertFalse(version.match("1.0", version.LESS, "1.0"))


if __name__ == "__main__":
    unittest.main()
