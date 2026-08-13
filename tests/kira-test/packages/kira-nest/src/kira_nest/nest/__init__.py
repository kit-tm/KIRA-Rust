from __future__ import annotations

import sys


# Workaround for [NeST#285](https://gitlab.com/nitk-nest/nest/-/work_items/285)
def silence_nest_errors(unraisable: sys.UnraisableHookArgs) -> None:
    err_str = str(unraisable.exc_value)
    if (
        (
            "can't register atexit after shutdown" in err_str
            and unraisable.exc_type is RuntimeError
        )
        or "sys.meta_path is None" in err_str
        and unraisable.exc_type is ImportError
    ):
        return
    # Fall back to default behavior for any other unraisable exceptions
    sys.__unraisablehook__(unraisable)


# Register the hook
sys.unraisablehook = silence_nest_errors
