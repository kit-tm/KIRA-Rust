from time import sleep

from kira_nest.nest.test import KIRATest
from nest.topology import Address


def test_connectivity(kirad_small_k, kira_topo, tmp_path, subtests):
    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    sleep(1)

    # checkup
    for n in test.topology.nodes:
        with subtests.test(msg="checkup", node=f"{n}"):
            assert n.is_up(), f"{n} is down"

    sleep(3)

    # pingall
    for x in test.topology.nodes:
        for y in test.topology.nodes:
            ip_y = y.node_id.to_node_ip()
            ip_y = Address(str(ip_y))
            with subtests.test(msg="ping", origin=f"{x}", destination=f"{y}"):
                assert x.ping(ip_y, packets=1, verbose=1), f"Ping {x} --> {y} failed!"
