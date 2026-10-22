// Headless AVPlayer harness: plays a URL for N seconds and reports what AVPlayer
// actually did. Exit code 0 = reached playing and the playhead advanced.
import AVFoundation
let url = URL(string: CommandLine.arguments[1])!
let secs = Double(CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "30")!
let item = AVPlayerItem(url: url)
let player = AVPlayer(playerItem: item)
player.isMuted = true
var obs: [NSKeyValueObservation] = []
func log(_ s: String) { print(String(format: "[%6.2f] ", Date().timeIntervalSince(start)) + s); fflush(stdout) }
let start = Date()
obs.append(item.observe(\.status) { i, _ in log("item.status=\(i.status.rawValue) err=\(String(describing: i.error))") })
obs.append(player.observe(\.timeControlStatus) { p, _ in log("timeControl=\(p.timeControlStatus.rawValue) wait=\(p.reasonForWaitingToPlay?.rawValue ?? "-")") })
NotificationCenter.default.addObserver(forName: .AVPlayerItemPlaybackStalled, object: item, queue: .main) { _ in log("STALLED") }
NotificationCenter.default.addObserver(forName: .AVPlayerItemFailedToPlayToEndTime, object: item, queue: .main) { n in log("FAILED \(String(describing: n.userInfo))") }
NotificationCenter.default.addObserver(forName: .AVPlayerItemNewErrorLogEntry, object: item, queue: .main) { _ in
  if let e = item.errorLog()?.events.last { log("ERRLOG status=\(e.errorStatusCode) domain=\(e.errorDomain) uri=\(e.uri ?? "-") \(e.errorComment ?? "")") } }
player.play()
var firstTime: Double? = nil
Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
  let t = player.currentTime().seconds
  if firstTime == nil && t > 0 { firstTime = t }
  let r = item.loadedTimeRanges.first?.timeRangeValue
  log(String(format: "t=%.2f buffered=%.2f rate=%.1f stalls=%d", t, r.map { $0.end.seconds - t } ?? -1, player.rate, item.accessLog()?.events.last?.numberOfStalls ?? -1))
}
DispatchQueue.main.asyncAfter(deadline: .now() + secs) {
  let ev = item.accessLog()?.events.last
  let adv = (firstTime.map { player.currentTime().seconds - $0 }) ?? 0
  log(String(format: "SUMMARY status=%d advanced=%.2fs stalls=%d bitrate=%.0f indicated=%.0f errors=%d", item.status.rawValue, adv, ev?.numberOfStalls ?? -1, ev?.observedBitrate ?? 0, ev?.indicatedBitrate ?? 0, item.errorLog()?.events.count ?? 0))
  exit(item.status == .readyToPlay && adv > secs * 0.5 ? 0 : 1)
}
RunLoop.main.run()
