from time import sleep

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
            ip_y = dst.node_id.to_node_ip()
            ip_y = Address(str(ip_y))
            with subtests.test(msg="ping", src=str(src), dst=str(dst)):
                assert src.ping(ip_y, packets=1, verbose=1)

    # traceroute
    for src in test.topology.nodes:
        for dst in test.topology.nodes:
            with subtests.test(msg="traceroute", src=str(dst), dst=str(dst)):
                assert test.traceroute(src, dst, verbose=False)
