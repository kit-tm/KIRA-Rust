import random
import time

import networkx as nx
from kira_nest.nest.test import KIRATest
from nest.topology import Address

CONVERGENCE_GRACE_SECS = 5
RETRIES = 5  # we retry pings 5 times because dynamic scenario may not have converged fully in edge cases


def test_dynamic_isolated_node_failure(
    kirad_small_k, kira_topo, subtests, conn_helpers
):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    node_list = list(test.topology.nodes)

    # Pick 30 % of the nodes randomly and fail them _individually_.
    candidates = random.sample(node_list, round(0.3 * len(node_list)))

    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    ### Network is assumed to be running okay here ###

    for isolated_node in candidates:
        isolated_tid = test.topology.tid(isolated_node)
        assert isolated_tid is not None
        ip_nip = isolated_node.node_id.to_node_ip()
        ip_nip = Address(str(ip_nip))

        ### Isolate node ###

        for v, link in test.topology.links[isolated_tid, ...]:
            print(f"{isolated_tid:>2} -✗- {v:>2}")
            link.down()

        print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
        time.sleep(CONVERGENCE_GRACE_SECS)

        # Node is isolated and still running
        for n in test.topology.nodes:
            with subtests.test(
                msg="checkup after isolation", n=str(n), isolated_node=isolated_node
            ):
                assert n.is_up()
        for src in test.topology.nodes:
            with subtests.test(
                msg="ping isolated (fail)",
                src=str(src),
                dst=isolated_node,
            ):
                assert not src.ping(ip_nip, packets=1, verbose=0), "fail ping isolated"

        # Ping functionality should remain unaffected for other nodes in the network
        conn_helpers.retry_sweep(
            test,
            conn_helpers.ping_check,
            test_msg=f"ping (isolated {isolated_node})",
        )

        ### Reconnect node ###

        for v, link in test.topology.links[isolated_tid, ...]:
            print(f"{isolated_tid:>2} -✔- {v:>2}")
            link.up()

        print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
        time.sleep(CONVERGENCE_GRACE_SECS)

        for n in test.topology.nodes:
            with subtests.test(
                msg="checkup after node recovered",
                n=str(n),
                recovered_node=isolated_node,
            ):
                assert n.is_up()

        # Connectivity is reinstated
        conn_helpers.retry_sweep(
            test,
            conn_helpers.ping_check,
            max_attempts=RETRIES,
            test_msg=f"ping (restored {isolated_node})",
        )


def test_dynamic_random_link_failure(kirad_small_k, kira_topo, subtests, conn_helpers):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    links_list = list(test.topology.links)

    # Pick 30 % of the links randomly and fail them _individually_.
    candidates = random.sample(links_list, round(0.3 * len(links_list)))

    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    ### Network is assumed to be running okay here ###

    for u, v, link in candidates:
        # Down link
        print()
        print(f"{u} -✗- {v}")
        link.down()

        print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
        time.sleep(CONVERGENCE_GRACE_SECS)

        for n in test.topology.nodes:
            with subtests.test(msg="checkup", link_failure=f"{u} -✗- {v}", n=str(n)):
                assert n.is_up()

        # Ping functionality should remain unaffected for nodes in the network
        conn_helpers.retry_sweep(
            test,
            conn_helpers.ping_check,
            max_attempts=RETRIES,
            test_msg=f"ping (link failure: {u} -✗- {v})",
        )

        ### Reconnect link ###
        print(f"{u} -✔- {v}")
        link.up()

        print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
        time.sleep(CONVERGENCE_GRACE_SECS)

        for n in test.topology.nodes:
            with subtests.test(
                msg="checkup",
                restored_link=f"{u} -✔- {v}",
                n=str(n),
            ):
                assert n.is_up()

        # Connectivity is reinstated
        conn_helpers.retry_sweep(
            test,
            conn_helpers.ping_check,
            max_attempts=RETRIES,
            test_msg=f"ping (link restored: {u} -✔- {v})",
        )


def test_dynamic_random_link_failures(kirad_small_k, kira_topo, subtests, conn_helpers):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    links_list = list(test.topology.links)

    # Pick 30 % of the links randomly and fail them _at the same time_.
    candidates = random.sample(links_list, round(0.3 * len(links_list)))

    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    ### Network is assumed to be running okay here ###

    # Down links
    for u, v, link in candidates:
        print()
        print(f"{u:>2} -✗- {v:>2}")
        link.down()

    print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
    time.sleep(CONVERGENCE_GRACE_SECS)

    for n in test.topology.nodes:
        with subtests.test(msg="checkup (links failed)", n=str(n)):
            assert n.is_up()

    # Ping functionality should remain unaffected for nodes in the network
    conn_helpers.retry_sweep(
        test,
        conn_helpers.ping_check,
        max_attempts=RETRIES,
        test_msg="ping (links failed)",
    )

    ### Reconnect links ###
    for u, v, link in candidates:
        print()
        print(f"{u:>2} -✔- {v:>2}")
        link.up()

    print(f"Waiting {CONVERGENCE_GRACE_SECS} seconds for network to converge...")
    time.sleep(CONVERGENCE_GRACE_SECS)

    for n in test.topology.nodes:
        with subtests.test(
            msg="checkup (links restored)",
            n=str(n),
        ):
            assert n.is_up()

    # Connectivity is reinstated
    conn_helpers.retry_sweep(
        test,
        conn_helpers.ping_check,
        max_attempts=RETRIES,
        test_msg="ping (links restored)",
    )


def test_dynamic_mst(kirad_small_k, kira_topo, subtests, conn_helpers):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    graph = test.topology.topology

    # Fail all links not part of MST based on random weights
    for u, v in graph.edges:
        graph[u][v]["mst_random_weight"] = random.random()
    mst = nx.minimum_spanning_tree(graph, weight="mst_random_weight")

    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    ### Network is assumed to be running okay here ###

    ### Fail Links ###

    print("Failing links not part of MST:")
    for u, v, link in test.topology.links:
        if (u, v) not in mst.edges():
            print(f"{u:>2} -✗- {v:>2}")
            link.down()
    print()

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
        test,
        conn_helpers.ping_check,
        max_attempts=RETRIES,
        test_msg="ping (mst failure)",
    )

    ### Restore Links ###

    print("Restoring all links:")
    for u, v, link in test.topology.links:
        if (u, v) not in mst.edges():
            print(f"{u:>2} -✔- {v:>2}")
            link.up()
    print()

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
        test,
        conn_helpers.ping_check,
        max_attempts=RETRIES,
        test_msg="ping (mst restore)",
    )
