import re
from mitmproxy import http
f = open("all.log", "a", buffering=1)
def response(flow: http.HTTPFlow):
    q = flow.request
    if "live.m3u8" not in q.path: return
    body = flow.response.get_text() or ""
    seq = re.search(r"MEDIA-SEQUENCE:(\d+)", body)
    full = re.findall(r"^(\d+)\.m4s$", body, re.M)
    last_full = int(full[-1]) if full else -1
    tail = body.split("\n%d.m4s\n" % last_full)[-1] if full else body
    nparts = len(re.findall(r"#EXT-X-PART:URI=", tail))
    rr = re.findall(r"RENDITION-REPORT:URI=\"\.\./(\w{8})[^\"]*\",LAST-MSN=(\d+)(?:,LAST-PART=(\d+))?", body)
    var = q.path.split("/")[3][:8] if q.path.count("/") > 3 else "master"
    f.write("%.3f-%.3f %s %s msn=%s part=%s -> full=%d parts=%d rr=%s\n" % (q.timestamp_start, flow.response.timestamp_end, var, flow.response.status_code, q.query.get("_HLS_msn"), q.query.get("_HLS_part"), last_full, nparts, ",".join("%s:%s/%s" % r for r in rr)))
