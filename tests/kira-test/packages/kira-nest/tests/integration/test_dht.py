import random

from kira_nest.nest.test import KIRATest

VALUE = "Psssst... This is a secret!"
KEYS = ["top", "abc", "_key_", "hello world"]


def test_dht_store_fetch(kirad_small_k, kira_topo, conn_helpers, subtests):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    node_list = list(test.topology.nodes)

    # Network appears functional
    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    for key in KEYS:
        # Choice random nodes as store/fetch targets
        n_store = random.choice(node_list)

        # Pick 30 % of the nodes randomly (max 30)
        ns_fetch = random.sample(node_list, min(round(0.3 * len(node_list)), 30))

        with subtests.test(msg="store", src=str(n_store), key=key, value=VALUE):
            store_res = n_store.api.store(key, VALUE)
            assert store_res is not None, "Store failed"

        for n_fetch in ns_fetch:
            with subtests.test(msg="fetch", src=str(n_fetch), key=key, value=VALUE):
                fetch_res = n_fetch.api.fetch(key)
                assert len(fetch_res) == 1, "Fetch failed"
                assert fetch_res[0] == VALUE, (
                    f"Unexpected return: Fetch '{key}'={VALUE} != res('{fetch_res[0]}')"
                )
