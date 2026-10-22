from mitmproxy import http
f = open("mitm.log", "a", buffering=1)
def response(flow: http.HTTPFlow):
    q = flow.request; r = flow.response
    dur = (r.timestamp_end or 0) - (q.timestamp_start or 0)
    n = len(r.raw_content or b"")
    f.write("%.3f %s %d %8d %5.2fs %s range=%r cr=%r\n" % (q.timestamp_start, q.http_version, r.status_code, n, dur, q.path, q.headers.get("range"), r.headers.get("content-range")))
