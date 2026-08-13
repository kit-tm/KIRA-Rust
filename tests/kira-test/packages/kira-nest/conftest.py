import os
import subprocess
import sys

from kira_nest.nest import silence_nest_errors
from kira_nest.unshare import probe_netns_cap, unshare_emulation


# automatically isolate tests using unshare(2)
def pytest_configure(config):
    if not probe_netns_cap():
        # download all dependencies beforehand
        subprocess.run(["cargo", "fetch", "--locked"], check=True)

        unshare_emulation()

        # no network access possible in the unshare isolation anyway
        # this is why we downloaded the dependencies prior
        os.environ["CARGO_NET_OFFLINE"] = "true"

    # Workaround for [NeST#285](https://gitlab.com/nitk-nest/nest/-/work_items/285)
    # Only effective with `-p no:unraisableexception`
    sys.unraisablehook = silence_nest_errors
