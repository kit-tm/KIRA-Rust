import pytest
from kira_common.domain import ENV_FORWARDING, NodeID, Path


@pytest.fixture(autouse=True)
def srv6_forwarding(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv(ENV_FORWARDING, "srv6")


def path(hops: int) -> Path:
    return Path(NodeID(bytes([index]) * NodeID.LENGTH) for index in range(1, hops + 1))


@pytest.mark.parametrize("hops", [1, 2, 3])
def test_short_paths_have_single_segment(hops: int):
    whole = path(hops)
    assert whole.segments() == (whole, None)


@pytest.mark.parametrize(
    ("hops", "first_hops", "second_hops"),
    [(4, 2, 2), (5, 3, 2), (6, 3, 3), (7, 4, 3), (9, 6, 3)],
)
def test_long_paths_have_two_segments(hops: int, first_hops: int, second_hops: int):
    whole = path(hops)
    first, second = whole.segments()

    assert first == whole[:first_hops]
    # the second segment starts with the last node of the first segment
    assert second == whole[first_hops - 1 :]
    assert len(second) == second_hops + 1


@pytest.mark.parametrize("hops", [1, 4, 7])
def test_gre_forwarding_uses_single_segment(monkeypatch: pytest.MonkeyPatch, hops: int):
    monkeypatch.setenv(ENV_FORWARDING, "gre")
    whole = path(hops)
    assert whole.segments() == (whole, None)
