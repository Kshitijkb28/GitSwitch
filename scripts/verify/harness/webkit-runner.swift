// Load a page into the system's WebKit — the engine a Tauri app runs in on
// macOS — inject a script before the page's own, run a step script inside the
// page and print what it returns. The browser harness proves the same things
// in Chrome; this proves them where the app actually runs.
//
//   webkit-runner <url> <bridge.js> <steps.js> [width height]
//
// The window is real and on screen (a hidden one is throttled: no layout
// callbacks, slow timers), does not take the keyboard, and closes by itself.
import Cocoa
import WebKit

let argv = CommandLine.arguments
guard argv.count >= 4, let target = URL(string: argv[1]),
      let bridge = try? String(contentsOfFile: argv[2], encoding: .utf8),
      let steps = try? String(contentsOfFile: argv[3], encoding: .utf8) else {
    FileHandle.standardError.write("usage: webkit-runner <url> <bridge.js> <steps.js> [width height]\n".data(using: .utf8)!)
    exit(64)
}
let width = argv.count > 4 ? (Double(argv[4]) ?? 1280) : 1280
let height = argv.count > 5 ? (Double(argv[5]) ?? 900) : 900

final class Runner: NSObject, WKNavigationDelegate {
    let target: URL
    let bridge: String
    let steps: String
    let width: Double
    let height: Double
    var web: WKWebView!
    var window: NSWindow!
    var started = false

    init(target: URL, bridge: String, steps: String, width: Double, height: Double) {
        self.target = target
        self.bridge = bridge
        self.steps = steps
        self.width = width
        self.height = height
    }

    func start() {
        let cfg = WKWebViewConfiguration()
        cfg.userContentController.addUserScript(
            WKUserScript(source: bridge, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        cfg.websiteDataStore = .nonPersistent() // nothing of this run is kept
        let frame = NSRect(x: 0, y: 0, width: width, height: height)
        web = WKWebView(frame: frame, configuration: cfg)
        web.navigationDelegate = self
        window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "GitSwitch layout check — closes by itself"
        window.contentView = web
        window.orderFrontRegardless()
        web.load(URLRequest(url: target))
        DispatchQueue.main.asyncAfter(deadline: .now() + 90) {
            self.finish("{\"error\":\"timed out after 90s\"}", 2)
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        if started { return }
        started = true
        web.callAsyncJavaScript(steps, arguments: [:], in: nil, in: .page) { result in
            switch result {
            case .success(let value): self.finish(String(describing: value), 0)
            case .failure(let error): self.finish("{\"error\":\"\(error.localizedDescription)\"}", 1)
            }
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        finish("{\"error\":\"navigation failed: \(error.localizedDescription)\"}", 1)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        finish("{\"error\":\"could not load the page: \(error.localizedDescription)\"}", 1)
    }

    func finish(_ out: String, _ code: Int32) {
        print(out)
        fflush(stdout)
        exit(code)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory) // no Dock icon, never the active app
let runner = Runner(target: target, bridge: bridge, steps: steps, width: width, height: height)
runner.start()
app.run()
