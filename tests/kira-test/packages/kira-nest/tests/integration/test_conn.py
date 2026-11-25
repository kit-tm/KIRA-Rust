from kira_nest.nest.test import KIRATest

# node pairs whose traceroute is checked (all pairs are pinged)
TRACEROUTE_SAMPLE = 200


def test_connectivity(kirad_small_k, kira_topo, conn_helpers):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)

    # All nodes come up within one second
    conn_helpers.checkup_timeout(test, timeout=1)

    # All nodes can reach each other using pings (checks fast forwarding layer)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")
    # and traceroutes don't fail
    conn_helpers.retry_sweep(
        test,
        lambda u, v: conn_helpers.traceroute_check(test, u, v),
        test_msg="traceroute",
        # each traceroute queries the forwarding state of every hop
        sample=TRACEROUTE_SAMPLE,
    )
