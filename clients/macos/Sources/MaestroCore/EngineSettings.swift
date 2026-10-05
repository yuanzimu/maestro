// EngineSettings.swift — 引擎设置：CCR 网关 + 硬预算（Sprint A 能力的 GUI 配置面）
//
// 持久化到 dataDir/engine-settings.json；拉起 daemon 时转成环境变量注入：
//   MAESTRO_CCR_URL / MAESTRO_CCR_TOKEN（网关；空 URL = 关闭）
//   MAESTRO_BUDGET_CENTS / MAESTRO_BUDGET_WALL_MS（预算；0 = 不限）
import Foundation

public struct EngineSettings: Codable, Equatable {
    /// 是否启用 CCR 网关（关闭 = worker 直连 provider）
    public var gatewayEnabled: Bool
    public var gatewayURL: String
    public var gatewayToken: String
    /// 花费硬顶（美分；0 = 不限）
    public var maxCostCents: Int
    /// 挂钟时长硬顶（分钟；0 = 不限）
    public var maxWallMinutes: Int

    public init(gatewayEnabled: Bool = true,
                gatewayURL: String = "http://127.0.0.1:3456",
                gatewayToken: String = "dummy",
                maxCostCents: Int = 500,
                maxWallMinutes: Int = 30) {
        self.gatewayEnabled = gatewayEnabled
        self.gatewayURL = gatewayURL
        self.gatewayToken = gatewayToken
        self.maxCostCents = maxCostCents
        self.maxWallMinutes = maxWallMinutes
    }
}

public final class EngineSettingsStore {
    public let dataDir: URL
    private let url: URL

    public init(dataDir: URL) {
        self.dataDir = dataDir
        self.url = dataDir.appendingPathComponent("engine-settings.json")
    }

    public func load() -> EngineSettings {
        guard let data = try? Data(contentsOf: url),
              let s = try? JSONDecoder().decode(EngineSettings.self, from: data)
        else { return EngineSettings() }
        return s
    }

    /// 原子写（tmp + rename）
    public func save(_ s: EngineSettings) throws {
        try FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
        let tmp = dataDir.appendingPathComponent("engine-settings.json.tmp")
        let data = try JSONEncoder().encode(s)
        try data.write(to: tmp, options: .atomic)
        try FileManager.default.replaceItemAt(url, withItemAt: tmp)
    }

    /// 转成注入 daemon 子进程的环境变量（对应 daemon main.rs 的读取键名）
    public func daemonEnv(for s: EngineSettings) -> [String: String] {
        // 网关：启用→URL（去尾部 /）；关闭→空串（daemon 见空串即禁用）
        var url = s.gatewayURL.trimmingCharacters(in: .whitespaces)
        if s.gatewayEnabled {
            while url.hasSuffix("/") { url.removeLast() }
        }
        let ccrURL = s.gatewayEnabled ? url : ""
        return [
            "MAESTRO_CCR_URL": ccrURL,
            "MAESTRO_CCR_TOKEN": s.gatewayToken,
            "MAESTRO_BUDGET_CENTS": String(max(0, s.maxCostCents)),
            "MAESTRO_BUDGET_WALL_MS": String(max(0, s.maxWallMinutes) * 60_000),
        ]
    }
}
