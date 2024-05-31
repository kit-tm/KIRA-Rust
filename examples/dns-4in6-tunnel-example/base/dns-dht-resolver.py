#!/usr/bin/python3

from typing import Optional, List

# parsing dht rest-api
import requests
import base64
from binascii import Error as Base64Error

# dns server
from dnslib import RR, QTYPE, RCODE, AAAA, DNSRecord
from dnslib.label import DNSBuffer
from dnslib.dns import OPCODE
from dnslib.server import DNSServer, BaseResolver, DNSLogger

import argparse
import os

import logging
logging.basicConfig(level=logging.DEBUG)


class DHTResolver(BaseResolver):
    def __init__(self, tlds: List[str] = ['.kira.internal.'], upstream_resolver: Optional[str] = None):
        self.tlds = tlds
        self.upstream_resolver = upstream_resolver

    def resolve(self, request: DNSRecord, handler):
        if request.header.opcode == OPCODE.QUERY:
            return self.resolve_query(request)

        if request.header.opcode == OPCODE.UPDATE:
            return self.resolve_update(request)

        if request.header.opcode == OPCODE.SOA:
            return self.resolve_soa(request)

        error_reply = request.reply()
        error_reply.header.rcode = RCODE.NOTIMP

    def resolve_query(self, request: DNSRecord):
        reply = request.reply(aa=1)
        # 1- strip request to find name and type (QNAME, QTYPE)
        # WE ONLY RESPOND TO ONE QUESTION
        qname = str(request.q.qname)
        qtype = request.q.qtype

        # Check if the TLD is internal and request type is AAAA
        if qtype not in [QTYPE.A, QTYPE.AAAA] or not any(qname.endswith(tld) for tld in self.tlds):
            upstream_reply = self.resolve_upstream(request)
            if upstream_reply is None:
                reply.header.rcode = RCODE.REFUSED
                return reply

            return upstream_reply

        # 2- ask DHT
        rrs = self.resolve_dht(qname)

        if rrs is None:
            logging.warning("Zone not found in DHT!")
            reply.header.rcode = RCODE.NXDOMAIN
            return reply

        # filter for actual asked QTYPE
        asked_present = False
        for rr in rrs:
            if not asked_present and rr.rtype == qtype:
                asked_present = True
                reply.add_answer(rr)
            else:
                reply.add_ar(rr)

        # we didn't received what we asked for :(
        if not asked_present:
            logging.warning(
                "Asked QTYPE %s not found but other QTYPES present")
            reply.header.rcode = RCODE.NXDOMAIN

        return reply

    def resolve_update(self, request: DNSRecord):
        reply = request.reply(aa=1)

        # rfc2136
        # zone => question (questions)
        # update => authority (auth)
        reply = request.reply()

        if len(request.questions) != 1 or request.q.qtype != QTYPE.SOA:
            logging.warning("Rejecting update without SOA ZTYPE: %s", request)
            reply.header.rcode = RCODE.FORMERR
            return reply
        zname = str(request.q.qname)
        if not any(zname.endswith(tld) for tld in self.tlds):
            logging.warning(
                "Rejecting update outside our authority: %s", request)
            reply.header.rcode = RCODE.NOTAUTH
            return reply

        # FIXME do not ignore the Prerequisite (Answer) section and class
        for update in request.auth:
            if update.rtype not in [QTYPE.A, QTYPE.AAAA]:
                logging.warning(
                    "Rejecting update of RR %s: Only accepting updates to A and AAAA: %s", update.rtype, request)
                reply.header.rcode = RCODE.FORMERR
                return reply
            if not str(update.rname).endswith(zname):
                logging.warning(
                    "UNAME not matching ZNAME %s: %s", zname, update.rname)
                reply.header.rcode = RCODE.FORMERR
                return reply

            self.update_dht(update)

        return reply

    def resolve_soa(self, request: DNSRecord):
        reply = request.reply()
        # TODO handle OPCODE.SOA
        return reply

    def resolve_upstream(self, request: DNSRecord) -> Optional[DNSRecord]:
        if self.upstream_resolver is None:
            logging.debug(
                "No upstream set. Can't ask upstream resolver for not internal address: %s", qname)
            return None
        logging.info(
            "Request for %s is not for an internal address, asking upstream resolver.", qname)
        proxy_r = request.send(self.upstream_resolver)
        return DNSRecord.parse(proxy_r)

    def resolve_dht(self, domain: str) -> Optional[List[RR]]:
        fetch_params = {"key": self.domain_to_key(domain)}
        try:
            fetch_r = requests.get("http://localhost:8080/dht/fetch",
                                   params=fetch_params)
        except requests.exceptions.ConnectionError:
            logging.error("Can't establish connection with DHT network api")
            return None
        fetch_status = fetch_r.status_code
        if fetch_status >= 200 and fetch_status < 300:
            pass
        else:
            logging.error(
                "Resolving service id %s not possible: %s",
                domain, fetch_r)
            return None

        try:
            json_r = fetch_r.json()

            # sanity checks
            if not hasattr(json_r, '__len__'):
                return None
            if len(json_r) == 0:
                return None

            # json array is b64 encoded
            rrs = (base64.b64decode(b64encoded_ip) for b64encoded_ip in json_r)

            # decode into RR type
            for rr in rrs:
                rr_buffer = DNSBuffer(data=rr)
                yield RR.parse(rr_buffer)
        except (requests.exceptions.JSONDecodeError, Base64Error) as decode_err:
            logging.error("Can't decode response: %s", decode_err)
            return None

    def update_dht(self, rr: RR) -> bool:
        store_params = {
            "restore": "true",
            "key": self.domain_to_key(rr.rname)
        }

        # serialize RR
        rr_buffer = DNSBuffer()
        rr.pack(rr_buffer)
        rr_buffer.offset = 0

        store_r = requests.post("http://localhost:8080/dht/store",
                                params=store_params, data=rr_buffer.data)
        store_status = store_r.status_code
        if store_status >= 200 and store_status < 300:
            logging.info(
                "Announced RR under id %s to the network.", store_params["key"])
        else:
            logging.error(
                "Announcing RR over DHT not possible: %s", store_r)

    def domain_to_key(self, domain: str) -> str:
        # TODO think of a better naming-scheme
        return f"dns://{domain}"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        prog="DHT-Resolver",
        description="""
            This DNS sever resolves A and AAAA RR of a tld using the DHT rest-api provided by the KIRA-routing daemon.

            For this to work the KIRA-node has to store it's own KIRA-IPv6 (fc00::) at the key <foo>.tld."""
    )

    parser.add_argument("tld", nargs='*', default=os.environ.get('DHTRESOLVER_TLD', 'kira.internal.').split(':'),
                        help="internal tld to resolve over the DHT, env: DHTRESOLVER_TLD, default: kira.internal.")
    parser.add_argument("--upstream", type=str, required=False, default=os.environ.get('DHTRESOLVER_UPSTREAM'), metavar='8.8.8.8',
                        help="Upstream DNS resolver for non internal addresses, env: DHTRESOLVER_UPSTREAM")

    args = parser.parse_args()

    PORT = 53
    resolver = DHTResolver(tlds=args.tld, upstream_resolver=args.upstream)
    dnslogger = DNSLogger(logf=lambda s: logging.info(s))
    dnsserver = DNSServer(resolver, port=PORT,
                          address="localhost", logger=dnslogger)
    try:
        dnsserver.start()
    except KeyboardInterrupt:
        pass
    finally:
        dnsserver.stop()
