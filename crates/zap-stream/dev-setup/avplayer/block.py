# Emulates the patched server: serves _HLS_msn/_HLS_part blocking reloads by
# polling the upstream playlist until patched playlist_has_part() is satisfied.
import asyncio, re, urllib.request
from mitmproxy import http
UP = "https://api-uk.zap.stream"

def has_part(body, want_msn, want_part):
    m = re.search(r"MEDIA-SEQUENCE:(\d+)", body)
    if not m: return False
    seq = int(m.group(1)); full = 0; trailing = 0; parts_of_wanted = None
    for line in body.splitlines():
        if line.startswith("#EXT-X-PART:"): trailing += 1
        elif line and not line.startswith("#"):
            if seq + full == want_msn: parts_of_wanted = trailing
            full += 1; trailing = 0
    inprog = seq + full
    if want_msn < inprog:
        if want_part is not None and parts_of_wanted and want_part >= parts_of_wanted and want_msn + 1 == inprog:
            return trailing > 0
        return True
    if want_msn == inprog and want_part is not None:
        return trailing > want_part
    return False

def fetch(path):
    req = urllib.request.Request(UP + path, headers={"Cache-Control": "no-cache", "User-Agent": "AppleCoreMedia/1.0.0 (Macintosh)"})
    return urllib.request.urlopen(req, timeout=10).read().decode()

async def request(flow: http.HTTPFlow):
    q = flow.request.query
    if "live.m3u8" not in flow.request.path or "_HLS_msn" not in q: return
    msn = int(q["_HLS_msn"]); part = int(q["_HLS_part"]) if "_HLS_part" in q else None
    base = flow.request.path.split("?")[0] + ("?vt=" + q["vt"] if "vt" in q else "")
    loop = asyncio.get_running_loop(); deadline = loop.time() + 6
    while True:
        body = await loop.run_in_executor(None, fetch, base)
        if has_part(body, msn, part) or loop.time() >= deadline: break
        await asyncio.sleep(0.05)
    flow.response = http.Response.make(200, body, {"Content-Type": "application/vnd.apple.mpegurl", "Cache-Control": "no-cache, no-store"})
