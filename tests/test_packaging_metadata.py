import tomllib
import unittest
from pathlib import Path


class PackagingMetadataTests(unittest.TestCase):
    def test_openhands_extra_is_guarded_to_supported_python_versions(self):
        pyproject = tomllib.loads(Path("pyproject.toml").read_text())
        openhands_deps = pyproject["project"]["optional-dependencies"]["openhands"]

        self.assertEqual(len(openhands_deps), 1)
        [dependency] = openhands_deps
        self.assertIn("python_version >= '3.12'", dependency)
        self.assertIn("python_version < '3.14'", dependency)


if __name__ == "__main__":
    unittest.main()
