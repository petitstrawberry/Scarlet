#!/usr/bin/env python3
"""Create a disposable Files visual-review fixture; print its new directory."""
from pathlib import Path
import tempfile
import zipfile


def main():
    root = Path(tempfile.mkdtemp(prefix="scarlet-files-visual-"))
    for name in (
        "Documents", "Downloads", "Pictures",
        "Project archive with a very long directory name", "日本語の資料と写真",
    ):
        (root / name).mkdir()
    (root / "Welcome.txt").write_text("Disposable Files visual-review fixture.\n")
    (root / "Project plan.md").write_text("# Project plan\n\nCheck navigation, resize, filtering and file details.\n")
    (root / "meeting-notes-日本語.txt").write_text("確認用のメモ\n")
    (root / "a-very-long-document-filename-for-wrapping-verification.txt").write_text("Long name\n")
    (root / ".hidden-note").write_text("Hidden-file toggle fixture\n")
    (root / "Documents/Overview.md").write_text("# Overview\n" * 70)
    (root / "Documents/Notes.txt").write_text("Fixture note\n" * 32)
    with zipfile.ZipFile(root / "Sample archive.zip", "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("Project/README.md", "# Sample archive\n")
        archive.writestr("Project/documents/日本語メモ.txt", "Archive note\n")
        archive.writestr("Project/documents/An intentionally long document name for the archive browser.txt", "Long name\n")
        archive.writestr("Project/images/cover.txt", "Image placeholder\n")
    print(root)


if __name__ == "__main__":
    main()
