import sys


# used for logging without disrupting stdout gml
def eprint(*args, **kwargs) -> None:
    print(*args, file=sys.stderr, **kwargs)
