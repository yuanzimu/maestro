// 渲染兜底：任一子树抛异常不再整树卸载（黑屏），降级为错误条 + 可刷新。
// 教训：一次 wire 不对齐（checkpoints 包装）导致整个 GUI 不可用。

import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}
interface State {
  error: string | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(e: unknown): State {
    return { error: e instanceof Error ? e.message : String(e) };
  }

  componentDidCatch(e: unknown, info: ErrorInfo) {
    console.error("[ErrorBoundary]", e, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="fatal">
          <div>
            <b>界面渲染出错</b>（任务引擎不受影响，仍在后台运行）
            <div className="fatal-err">{this.state.error}</div>
          </div>
          <div className="btns">
            <button onClick={() => this.setState({ error: null })}>重试</button>
            <button className="primary" onClick={() => location.reload()}>
              刷新界面
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
