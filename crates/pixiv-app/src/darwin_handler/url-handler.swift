import Cocoa
import Foundation

final class AppDelegate: NSObject, NSApplicationDelegate {
    override init() {
        super.init()
        NSAppleEventManager.shared().setEventHandler(self, andSelector: #selector(handleGetURLEvent(_:withReplyEvent:)), forEventClass: AEEventClass(kInternetEventClass), andEventID: AEEventID(kAEGetURL))
    }

	@objc func handleGetURLEvent(_ event: NSAppleEventDescriptor, withReplyEvent replyEvent: NSAppleEventDescriptor) {
		guard let callbackURL = event.paramDescriptor(forKeyword: keyDirectObject)?.stringValue else {
			NSApp.terminate(nil)
			return
		}
		runCallbackHandler(callbackURL)
		NSApp.terminate(nil)
	}

	private func runCallbackHandler(_ callbackURL: String) {
		guard let manifestURL = manifestURL(),
			  let data = try? Data(contentsOf: manifestURL),
			  let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
			  let executable = object["executable_path"] as? String,
			  !executable.isEmpty else {
			return
		}
		let process = Process()
		process.executableURL = URL(fileURLWithPath: executable)
		process.arguments = ["auth", "_callback", callbackURL]
		if let homeDirectory = object["home_directory"] as? String, !homeDirectory.isEmpty {
			var environment = ProcessInfo.processInfo.environment
			environment["HOME"] = homeDirectory
			environment["USERPROFILE"] = homeDirectory
			process.environment = environment
		}
		try? process.run()
	}

	private func manifestURL() -> URL? {
		// LaunchServices 不会继承 CLI 进程的 HOME。先读随已注册 app 一同写入
		// 的私有 manifest，使隔离 HOME 仍能回调正确 binary；旧版本只保存的
		// 用户目录副本仍作为兼容回退。二者都不含 bearer secret。
		if let resourceURL = Bundle.main.resourceURL {
			let bundleManifest = resourceURL.appendingPathComponent("handler-manifest.json")
			if FileManager.default.isReadableFile(atPath: bundleManifest.path) {
				return bundleManifest
			}
		}
		return URL(fileURLWithPath: NSHomeDirectory())
			.appendingPathComponent(".pixiv-cli/url-handler/handler-manifest.json")
	}
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
