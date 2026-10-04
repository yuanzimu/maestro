// UnixSocket.swift — AF_UNIX 流式连接（JSONL 线协议：一行请求 / 一行响应）
import Foundation
import Darwin

public enum UnixSocketError: Error, CustomStringConvertible {
    case socketCreateFailed(Int32)
    case connectFailed(String, Int32)
    case writeFailed(Int32)
    case readFailed(Int32)
    case timedOut
    case closed

    public var description: String {
        switch self {
        case .socketCreateFailed(let e): return "创建 socket 失败 (errno=\(e))"
        case .connectFailed(let p, let e): return "连接 \(p) 失败 (errno=\(e))"
        case .writeFailed(let e): return "写入失败 (errno=\(e))"
        case .readFailed(let e): return "读取失败 (errno=\(e))"
        case .timedOut: return "响应超时 —— daemon 无应答"
        case .closed: return "连接已关闭"
        }
    }
}

public final class UnixSocketConnection {
    private let fd: Int32
    private var buffer = Data()
    private var closed = false

    /// - Parameter receiveTimeout: 读超时（秒）。API 请求/响应用（防 daemon
    ///   无应答时调用方永久阻塞 —— GUI 冻结）；EventStream 常驻订阅传 nil
    ///   （靠 close() 打断阻塞读）
    public init(path: String, receiveTimeout: TimeInterval? = nil) throws {
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
            Darwin.close(fd)
            throw UnixSocketError.connectFailed(path, 0)
        }
        let r = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                connect(fd, sa, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard r == 0 else {
            let e = errno
            Darwin.close(fd)
            throw UnixSocketError.connectFailed(path, e)
        }
        if let t = receiveTimeout {
            var tv = timeval(tv_sec: time_t(t),
                             tv_usec: suseconds_t((t - floor(t)) * 1_000_000))
            _ = setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        }
    }

    deinit {
        if !closed { Darwin.close(fd) }
    }

    /// 主动关闭（shutdown 打断阻塞中的 read，线程才能退出 —— EventStream.stop 依赖）
    public func close() {
        guard !closed else { return }
        closed = true
        _ = Darwin.shutdown(fd, Int32(SHUT_RDWR))
        Darwin.close(fd)
    }

    /// 探测端点是否可连（daemon 存活检查）
    public static func canConnect(path: String) -> Bool {
        (try? UnixSocketConnection(path: path)) != nil
    }

    public func writeAll(_ data: Data) throws {
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
    public func readLine() throws -> Data? {
        while true {
            if let idx = buffer.firstIndex(of: 0x0A) {
                let line = buffer.subdata(in: buffer.startIndex..<idx)
                buffer.removeSubrange(buffer.startIndex...idx)
                buffer = Data(buffer)
                return line
            }
            var chunk = [UInt8](repeating: 0, count: 65536)
            let n = read(fd, &chunk, chunk.count)
            if n < 0 {
                // SO_RCVTIMEO 到点：EAGAIN/EWOULDBLOCK —— 明确报超时（而非笼统读失败）
                if errno == EAGAIN || errno == EWOULDBLOCK {
                    throw UnixSocketError.timedOut
                }
                throw UnixSocketError.readFailed(errno)
            }
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
