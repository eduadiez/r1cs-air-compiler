"""Add a member to the `members = [` list of a Cargo workspace manifest.

Usage: add_member.py CARGO_TOML MEMBER
"""
import sys


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__.strip().splitlines()[-1])
    path, member = sys.argv[1:]
    with open(path, encoding="utf-8") as manifest:
        lines = manifest.read().splitlines(keepends=True)
    for index, line in enumerate(lines):
        if line.startswith("members = ["):
            lines.insert(index + 1, f'  "{member}",\n')
            break
    else:
        sys.exit(f"{path}: no 'members = [' line")
    with open(path, "w", encoding="utf-8") as manifest:
        manifest.writelines(lines)


if __name__ == "__main__":
    main()
