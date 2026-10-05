// CommandFilter.swift — 命令面板的纯逻辑：模糊过滤 + 键盘索引移动
//
// 从 CommandPaletteController 抽出，便于用 mock 数据单测验证
// （不依赖 AppKit/窗口，纯 Foundation）。
import Foundation

/// 可被命令面板检索的条目（产品 CommandItem 的数据部分）
public struct FilterableCommand: Equatable {
    public let title: String
    public let subtitle: String
    public init(title: String, subtitle: String) {
        self.title = title
        self.subtitle = subtitle
    }
}

/// 键盘移动方向
public enum MoveDirection {
    case up, down
}

/// 可按标题/副标题匹配的条目（产品 CommandItem 的数据约束）
public protocol CommandMatchable {
    var title: String { get }
    var subtitle: String { get }
}

extension FilterableCommand: CommandMatchable {}

/// 命令面板纯逻辑集合
public enum CommandFilter {
    /// 单条是否命中 query（大小写不敏感、title/subtitle 任一包含）
    public static func matches(_ item: CommandMatchable, query: String) -> Bool {
        item.title.range(of: query, options: .caseInsensitive) != nil
            || item.subtitle.range(of: query, options: .caseInsensitive) != nil
    }

    /// 泛型过滤：任何满足 CommandMatchable 的条目（产品侧的 CommandItem 可直接用，
    /// 不必先转成 FilterableCommand）；query 空白 = 全部返回，保持原始顺序。
    public static func filter<T: CommandMatchable>(_ items: [T], query: String) -> [T] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return items }
        return items.filter { matches($0, query: q) }
    }

    /// 键盘 ↑↓ 移动索引：不越界（0...count-1）；count=0 恒返回 0。
    public static func move(index: Int, direction: MoveDirection, count: Int) -> Int {
        guard count > 0 else { return 0 }
        let maxIndex = count - 1
        switch direction {
        case .up: return max(0, index - 1)
        case .down: return min(maxIndex, index + 1)
        }
    }
}
