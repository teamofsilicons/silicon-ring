import Foundation

/// Debug-only breadcrumbs for call flows that run while nobody can watch the device, such as a
/// call answered from a VoIP push. Release builds compile every call to nothing.
/// Read with `xcrun devicectl device copy from --domain-type appDataContainer
/// --domain-identifier com.teamofsilicons.ring --source Library/Caches/ring-dev.log …`.
enum DevLog {
    #if DEBUG
    private static let queue = DispatchQueue(label: "com.teamofsilicons.ring.devlog")
    private static let url: URL? = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
        .appendingPathComponent("ring-dev.log")
    #endif

    static func log(_ message: @autoclosure () -> String) {
        #if DEBUG
        let line = "\(Date().ISO8601Format(.iso8601.time(includingFractionalSeconds: true))) \(message())\n"
        queue.async {
            guard let url, let data = line.data(using: .utf8) else { return }
            if let handle = try? FileHandle(forWritingTo: url) {
                handle.seekToEndOfFile()
                handle.write(data)
                try? handle.close()
            } else {
                try? data.write(to: url)
            }
        }
        #endif
    }
}
