import os
import subprocess

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
