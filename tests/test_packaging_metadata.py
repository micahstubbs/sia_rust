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

    def test_project_urls_point_to_rust_port_repo(self):
        # User-facing package metadata must point to the Rust-port repo, not the
        # upstream Python repo. See issue #117.
        pyproject = tomllib.loads(Path("pyproject.toml").read_text())
        urls = pyproject["project"]["urls"]

        expected = "https://github.com/micahstubbs/sia_rust"
        self.assertEqual(urls["Homepage"], expected)
        self.assertEqual(urls["Repository"], expected)
        for value in urls.values():
            self.assertNotIn("hexo-ai/sia", value)


if __name__ == "__main__":
    unittest.main()
