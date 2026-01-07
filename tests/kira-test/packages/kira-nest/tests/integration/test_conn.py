from time import sleep

from kira_common import TOPOS_DIR
from kira_nest.nest.test import KIRATest
from nest.topology import Address


def test_connectivity(kirad_small_k, kira_topo, subtests):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    sleep(1)

    # checkup
    for n in test.topology.nodes:
        with subtests.test(msg="checkup", n=str(n)):
            assert n.is_up()

    sleep(3)

    # pingall
    for src in test.topology.nodes:
        for dst in test.topology.nodes:
            if src == dst:
                break

            ip_dst = dst.node_id.to_node_ip()
            ip_dst = Address(str(ip_dst))
            with subtests.test(msg="ping", src=str(src), dst=str(dst)):
                assert src.ping(ip_dst, packets=1, verbose=1)

    # traceroute
    for src in test.topology.nodes:
        for dst in test.topology.nodes:
            with subtests.test(msg="traceroute", src=str(src), dst=str(dst)):
                assert test.traceroute(src, dst, verbose=False)

    # test link up/down of k1
    minimal_topo = TOPOS_DIR / "minimal-small-k.gml"
    if kira_topo == minimal_topo:
        isolating_tid = "1"
        isolating_node = test.topology.nodes[isolating_tid]
        ip_tid = isolating_node.node_id.to_node_ip()
        ip_tid = Address(str(ip_tid))

        # isolate tid
        for _, link in test.topology.links[isolating_tid, ...]:
            link.down()
        sleep(5)

        # test if connectivity (of other nodes) is unaffected
        for n in test.topology.nodes:
            with subtests.test(
                msg=f"checkup after isolation of {isolating_node}", n=str(n)
            ):
                assert n.is_up()
        for src in test.topology.nodes:
            with subtests.test(
                msg=f"ping isolated {isolating_node}",
                src=str(src),
                dst=isolating_node,
            ):
                assert not src.ping(ip_tid, packets=1, verbose=0), "fail ping isolated"
        for src in test.topology.nodes:
            for dst in test.topology.nodes:
                if isolating_node in (src, dst):
                    break  # skip isolated tid
                if src == dst:
                    break

                ip_dst = dst.node_id.to_node_ip()
                ip_dst = Address(str(ip_dst))
                with subtests.test(msg="ping", src=str(src), dst=str(dst)):
                    assert src.ping(ip_dst, packets=1, verbose=1), (
                        "unaffected connectivity"
                    )
        for src in test.topology.nodes:
            for dst in test.topology.nodes:
                if isolating_node in (src, dst):
                    break
                if src == dst:
                    break

                with subtests.test(
                    msg=f"traceroute with isolated {isolating_node}",
                    src=str(src),
                    dst=str(dst),
                ):
                    assert test.traceroute(src, dst, verbose=False), (
                        "unaffected connectivity"
                    )

        # connect tid
        for _, link in test.topology.links[isolating_tid, ...]:
            link.up()
        sleep(5)

        # test if connectivity is reinstated
        for n in test.topology.nodes:
            with subtests.test(
                msg=f"checkup after restored connectivity of {isolating_node}",
                n=str(n),
            ):
                assert n.is_up()
        for src in test.topology.nodes:
            with subtests.test(
                msg=f"ping restored {isolating_node}",
                src=str(src),
                dst=isolating_node,
            ):
                assert src.ping(ip_tid, packets=1, verbose=0), (
                    f"restored {isolating_node} reachable"
                )
        for src in test.topology.nodes:
            for dst in test.topology.nodes:
                if isolating_node in (src, dst):
                    break  # skip isolated tid
                if src == dst:
                    break

                ip_dst = dst.node_id.to_node_ip()
                ip_dst = Address(str(ip_dst))
                with subtests.test(
                    msg="ping after restored connectivity", src=str(src), dst=str(dst)
                ):
                    assert src.ping(ip_dst, packets=1, verbose=1), (
                        "unaffected connectivity"
                    )
        for src in test.topology.nodes:
            for dst in test.topology.nodes:
                if src == dst:
                    break
                with subtests.test(
                    msg=f"traceroute after restored connectivity of {isolating_node}",
                    src=str(src),
                    dst=str(dst),
                ):
                    assert test.traceroute(src, dst, verbose=False)
