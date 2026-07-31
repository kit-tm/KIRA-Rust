import random
import time

import networkx as nx
from kira_nest.nest.test import KIRATest
from nest.topology import Address

CONVERGENCE_GRACE_SECS = 20


def test_dynamic_one_isolated_failure(kirad_small_k, kira_topo, subtests, conn_helpers):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    node_list = list(test.topology.nodes)

    # Pick 30 % of the nodes randomly (but max 10) and fail them _individually_.
    candidates = random.sample(node_list, min(round(0.3 * len(node_list)), 10))

    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    ### Network is assumed to be running okay here ###

    def ping_check_isolated(src, dst, isolated_node):
        if isolated_node in (src, dst):
            return True  # skip isolated tid
        return conn_helpers.ping_check(src, dst)

    # Not isolated in subtests because broken network on previous test is likely
    # affecting consecutive test

    for isolated_node in candidates:
        isolated_tid = test.topology.tid(isolated_node)
        assert isolated_tid is not None
        ip_tid = isolated_node.node_id.to_node_ip()
        ip_tid = Address(str(ip_tid))

        ### Isolate node ###

        for _, link in test.topology.links[isolated_tid, ...]:
            link.down()

        print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
        time.sleep(CONVERGENCE_GRACE_SECS)

        # Node is isolated and still running
        for n in test.topology.nodes:
            with subtests.test(
                msg=f"checkup after isolation of {isolated_node}", n=str(n)
            ):
                assert n.is_up()
        for src in test.topology.nodes:
            with subtests.test(
                msg=f"ping isolated {isolated_node}",
                src=str(src),
                dst=isolated_node,
            ):
                assert not src.ping(ip_tid, packets=1, verbose=0), "fail ping isolated"

        # Ping functionality should remain unaffected for other nodes in the network
        conn_helpers.retry_sweep(
            test,
            lambda s, d: ping_check_isolated(s, d, isolated_node),  # noqa: B023
            test_msg=f"ping (isolated {isolated_node})",
        )

        ### Reconnect node ###

        for _, link in test.topology.links[isolated_tid, ...]:
            link.up()

        print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
        time.sleep(CONVERGENCE_GRACE_SECS)

        for n in test.topology.nodes:
            with subtests.test(
                msg=f"checkup after restored connectivity of {isolated_node}",
                n=str(n),
            ):
                assert n.is_up()

        # Connectivity is reinstated
        conn_helpers.retry_sweep(
            test,
            conn_helpers.ping_check,
            test_msg=f"ping (after restored connectivity of {isolated_node}",
        )


def test_dynamic_mst(kirad_small_k, kira_topo, subtests, conn_helpers):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    graph = test.topology.topology

    # Fail all links not part of MST based on random weights
    for u, v in graph:
        graph[u][v]["mst_random_weight"] = random.random()
    mst = nx.minimum_spanning_tree(graph, weight="mst_random_weight")

    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    ### Network is assumed to be running okay here ###

    ### Fail Links ###

    for u, v, link in test.topology.links:
        if (u, v) not in mst.edges():
            print(f"Failing link {u} -- {v} (not part of MST")
            link.down()

    print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
    time.sleep(CONVERGENCE_GRACE_SECS)

    # All nodes are still running
    for n in test.topology.nodes:
        with subtests.test(
            msg="checkup (mst failure)",
            n=str(n),
        ):
            assert n.is_up()

    # Ping functionality should remain unaffected (MST afterall)
    conn_helpers.retry_sweep(
        test, conn_helpers.ping_check, test_msg="ping (mst failure)"
    )

    ### Restore Links ###

    for u, v, link in test.topology.links:
        if (u, v) not in mst.edges():
            print(f"Restore link {u} -- {v}")
            link.up()

    print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
    time.sleep(CONVERGENCE_GRACE_SECS)

    # All nodes are still running
    for n in test.topology.nodes:
        with subtests.test(
            msg="checkup (mst failure restore)",
            n=str(n),
        ):
            assert n.is_up()

    # Ping functionality unaffected
    conn_helpers.retry_sweep(
        test, conn_helpers.ping_check, test_msg="ping (mst restore)"
    )
