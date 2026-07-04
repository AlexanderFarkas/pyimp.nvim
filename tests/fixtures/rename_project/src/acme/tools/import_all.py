import importlib
import pathlib
import sys


def main() -> int:
    src = pathlib.Path(sys.argv[1]).resolve()
    package = sys.argv[2]
    package_dir = src.joinpath(*package.split('.'))
    failures = []
    for path in sorted(package_dir.rglob('*.py')):
        rel = path.relative_to(src).with_suffix('')
        parts = list(rel.parts)
        if parts[-1] == '__init__':
            parts.pop()
        if not parts:
            continue
        module = '.'.join(parts)
        try:
            importlib.import_module(module)
        except Exception as exc:  # noqa: BLE001 - fixture diagnostic script
            failures.append(f'{module}: {type(exc).__name__}: {exc}')
    if failures:
        print('\n'.join(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
