// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "MaestroGui",
    platforms: [.macOS(.v13)],
    targets: [
        // 客户端核心（协议/事件流/daemon 管理/状态）—— GUI 与场景测试共用
        .target(
            name: "MaestroCore",
            path: "Sources/MaestroCore"
        ),
        .executableTarget(
            name: "MaestroGui",
            dependencies: ["MaestroCore"],
            path: "Sources/MaestroGui"
        ),
        // 100 场景验收 harness：复用客户端自身代码对真实 daemon 做全链路测试
        .executableTarget(
            name: "ScenarioRunner",
            dependencies: ["MaestroCore"],
            path: "Sources/ScenarioRunner"
        ),
        // ⌘K 纯逻辑 mock 验证：模糊搜索 + ↑↓ 快捷键（无 XCTest 环境也能跑：swift run PaletteMockCheck）
        .executableTarget(
            name: "PaletteMockCheck",
            dependencies: ["MaestroCore"],
            path: "Sources/PaletteMockCheck"
        ),
    ]
)
