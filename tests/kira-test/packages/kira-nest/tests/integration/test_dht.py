from time import sleep

from kira_nest.nest.test import KIRATest

VALUE = "Psssst... This is a secret!"
KEYS = ["top", "abc", "_key_", "hello world"]


def test_dht_store_fetch(kirad_small_k, kira_topo):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    sleep(1)

    n0 = next(iter(test.topology.nodes))
    assert n0.is_up(), f"{n0} is down"

    for key in KEYS:
        store_res = n0.api.store(key, VALUE)
        assert store_res is not None, f"Store '{key}' failed on {n0}"

        fetch_res = n0.api.fetch(key)
        assert len(fetch_res) == 1, f"Fetch '{key}' failed on {n0}"
        assert fetch_res[0] == VALUE, (
            f"{n0}: Fetch '{key}'={VALUE} != res('{fetch_res[0]}')"
        )
