import copy
import shlex
from argparse import Action, ArgumentError, ArgumentParser, ArgumentTypeError, Namespace
from collections.abc import Callable, Sequence
from functools import wraps
from typing import Any

from kira_common.domain import NODE_IP_SN, PATH_IP_SN, NodeID, NodeIP, PathIP

from kira_nest.domain.topology import KIRANodeView


def with_argparser[S, R](
    parser: ArgumentParser | str,
) -> Callable[[Callable[[S, Namespace], R]], Callable[[S, str | Namespace], R | None]]:
    """
    Decorator that parses arguments with an ArgumentParser first
    before calling the method with the parsed arguments dictionary.
    """

    def decorator(
        f: Callable[[S, Namespace], R],
    ) -> Callable[[S, str | Namespace], R | None]:
        if isinstance(parser, ArgumentParser):
            static_parser = copy.deepcopy(parser)

            # set function (do_ prefix stripped) as program name for correct help
            static_parser.prog = f.__name__.split("_")[-1]
            active_parser = static_parser
        else:
            active_parser = None

        @wraps(f)  # copy function meta-data like name, docs, ...
        def wrapper(self: S, arg: str | Namespace) -> R | None:
            nonlocal active_parser
            if active_parser is None:
                if isinstance(parser, str):
                    # dynamically look up parser
                    active_parser = getattr(self, parser)

                    active_parser.prog = f.__name__.split("_")[-1]
                    wrapper.__doc__ = active_parser.format_help()
                else:
                    raise ValueError("Unable to obtain active ArgumentParser")

            if type(arg) is str:
                args = shlex.split(arg)
                try:
                    parsed_args = active_parser.parse_args(args)
                except SystemExit:  # prevent exit of argument parser
                    return None
                except (ArgumentError, ArgumentTypeError) as e:
                    print(e)
                    return None
            elif isinstance(arg, Namespace):
                parsed_args = arg
            else:
                raise TypeError(
                    f"arg has to be str of Namespace but is type '{type(arg)}'"
                )

            return f(self, parsed_args)

        if active_parser is None:
            # setup for @init_argparser
            wrapper._parser = parser  # pyright: ignore
        else:
            # overwrite doc string of f with help of parser so Cmd can pick it up
            wrapper.__doc__ = active_parser.format_help()

        return wrapper

    return decorator


def init_argparser[C](cls: C) -> C:
    """
    Queries each 'do_X' method for a potential attached Argparser
    function to update the docs of the method accordingly.
    """

    init = cls.__init__

    @wraps(init)
    def init_wrapper(self: object, *args, **kwargs) -> None:
        init(self, *args, **kwargs)  # pyright: ignore

        for name, f in list(cls.__dict__.items()):
            if not name.startswith("do_"):
                continue
            if not hasattr(f, "_parser"):
                continue

            parser: ArgumentParser = getattr(self, f._parser)
            parser.prog = f.__name__.split("_")[-1]
            f.__doc__ = parser.format_help()

    cls.__init__ = init_wrapper  # pyright: ignore

    return cls


class StoreNode(Action):
    def __init__(self, node_view: KIRANodeView, *args, **kwargs) -> None:  # noqa: ANN002, ANN003
        self.node_view = node_view
        super().__init__(*args, **kwargs)

        assert self.nargs is None or self.nargs == "?"
        assert self.const is None
        assert self.type is None
        assert self.choices is None

    def __call__(
        self,
        parser: ArgumentParser,
        namespace: Namespace,
        values: str | Sequence[Any] | None,
        option_string: str | None = None,
    ) -> None:
        _ = parser
        if values is None:
            if self.nargs != "?":
                raise ValueError("Node parameter missing")
            setattr(namespace, self.dest, None)
            return

        assert type(values) is str
        assert option_string is None

        # try to parse values as NodeID, NodeIP or just use as plain "name"
        try:
            nid = NodeID.fromhex(values)
            try:
                node = self.node_view.by_nid(nid)
            except KeyError as e:
                raise ArgumentTypeError(f"Unknown Node-ID: {nid}") from e
        except ValueError:
            try:
                nip = NodeIP(values)
                try:
                    node = self.node_view.by_nip(nip)
                except KeyError as e:
                    raise ArgumentTypeError(f"Unknown Node-IP: {nip}") from e
            except ValueError:
                node = self.node_view.by_name(values)
                if node is None:
                    raise ArgumentTypeError(f"Unknown Topology ID: {values}") from None

        setattr(namespace, self.dest, node)

    def format_usage(self) -> str:
        return "Topology ID | Node-ID | Node-IP"


class StoreKIRAIP(Action):
    def __init__(self, *args, **kwargs) -> None:  # noqa: ANN002, ANN003
        super().__init__(*args, **kwargs)

        assert self.nargs is None or self.nargs == "?"
        assert self.const is None
        assert self.type is None
        assert self.choices is None

    def __call__(
        self,
        parser: ArgumentParser,
        namespace: Namespace,
        values: str | Sequence[Any] | None,
        option_string: str | None = None,
    ) -> None:
        _ = parser
        if values is None:
            if self.nargs != "?":
                raise ValueError("KIRAIP parameter missing")

            setattr(namespace, self.dest, None)
            return

        assert type(values) is str
        assert option_string is None

        ip_str = values.split("/")[0]  # remove subnet mask if present

        try:
            ip = NodeIP(ip_str)
        except ValueError:
            ip = PathIP(ip_str)
        setattr(namespace, self.dest, ip)

    def format_usage(self) -> str:
        return f"{NODE_IP_SN} | {PATH_IP_SN}"
