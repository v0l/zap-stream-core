import re
from mitmproxy import http
# Emulate the patched get_range: an RFC 8673 end (>= file size) means "to EOF".
def request(flow: http.HTTPFlow):
    r = flow.request.headers.get("range")
    m = r and re.fullmatch(r"bytes=(\d+)-(\d{12,})", r)
    if m: flow.request.headers["range"] = "bytes=%s-" % m.group(1)
