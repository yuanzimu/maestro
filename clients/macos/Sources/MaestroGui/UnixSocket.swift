// UnixSocket.swift — AF_UNIX 流式连接（JSONL 线协议：一行请求 / 一行响应）
import Foundation
import Darwin

enum UnixSocketError: Error, CustomStringConvertible {
    case socketCreateFailed(Int32)
    case connectFailed(String, Int32)
    case writeFailed(Int32)
    case readFailed(Int32)
    case closed

    var description: String {
        switch self {
        case .socketCreateFailed(let e): return "创建 socket 失败 (errno=\(e))"
        case .connectFailed(let p, let e): return "连接 \(p) 失败 (errno=\(e))"
        case .writeFailed(let e): return "写入失败 (errno=\(e))"
        case .readFailed(let e): return "读取失败 (errno=\(e))"
        case .closed: return "连接已关闭"
        }
    }
}

final class UnixSocketConnection {
    private let fd: Int32
    private var buffer = Data()

    init(path: String) throws {
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw UnixSocketError.socketCreateFailed(errno) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let copied = path.withCString { cstr -> Bool in
            let len = strlen(cstr)
            guard len < MemoryLayout.size(ofValue: addr.sun_path) else { return false }
            withUnsafeMutableBytes(of: &addr.sun_path) { dst in
                dst.copyBytes(from: UnsafeRawBufferPointer(start: cstr, count: len + 1))
            }
            return true
        }
        guard copied else {
            close(fd)
            throw UnixSocketError.connectFailed(path, 0)
        }
        let r = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                connect(fd, sa, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard r == 0 else {
            let e = errno
            close(fd)
            throw UnixSocketError.connectFailed(path, e)
        }
    }

    deinit { close(fd) }

    /// 探测端点是否可连（daemon 存活检查）
    static func canConnect(path: String) -> Bool {
        (try? UnixSocketConnection(path: path)) != nil
    }

    func writeAll(_ data: Data) throws {
        var offset = 0
        while offset < data.count {
            let n: Int = data.withUnsafeBytes { raw in
                guard let base = raw.baseAddress else { return 0 }
                return Darwin.write(fd, base.assumingMemoryBound(to: UInt8.self).advanced(by: offset),
                                    data.count - offset)
            }
            if n <= 0 { throw UnixSocketError.writeFailed(errno) }
            offset += n
        }
    }

    /// 读一行（按 \n 分隔）。返回 nil = 对端关闭且无残余数据。
    func readLine() throws -> Data? {
        while true {
            if let idx = buffer.firstIndex(of: 0x0A) {
                let line = buffer.subdata(in: buffer.startIndex..<idx)
                buffer.removeSubrange(buffer.startIndex...idx)
                buffer = Data(buffer)
                return line
            }
            var chunk = [UInt8](repeating: 0, count: 65536)
            let n = read(fd, &chunk, chunk.count)
            if n < 0 { throw UnixSocketError.readFailed(errno) }
            if n == 0 {
                if buffer.isEmpty { return nil }
                let rest = buffer
                buffer = Data()
                return rest
            }
            buffer.append(contentsOf: chunk[0..<n])
        }
    }
}
