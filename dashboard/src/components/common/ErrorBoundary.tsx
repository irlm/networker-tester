import { Component, type ErrorInfo, type ReactNode } from 'react';

interface ErrorBoundaryProps {
  children: ReactNode;
  /**
   * When this value changes (e.g. route path), a crashed boundary resets so
   * navigating away from a broken page recovers without a full reload.
   */
  resetKey?: string;
}

interface ErrorBoundaryState {
  error: Error | null;
}

/**
 * Top-level React error boundary. Before this existed, a single render throw
 * (e.g. the agents contract drift, v0.28.36) unmounted the entire app and
 * left users staring at a black screen. Now every crash lands on a
 * terminal-styled panel with a reload action instead.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Surface in the console for bug reports — the UI shows a short form.
    console.error('[ErrorBoundary]', error, info.componentStack);
  }

  componentDidUpdate(prevProps: ErrorBoundaryProps) {
    if (this.state.error && prevProps.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="min-h-[60vh] flex items-center justify-center p-6 bg-[var(--bg-base)]" role="alert">
        <div className="w-full max-w-lg border border-red-500/30 rounded bg-[var(--bg-surface)]">
          <div className="px-4 py-2.5 border-b border-red-500/20 flex items-center gap-2">
            <span className="w-2 h-2 rounded-full bg-red-400" aria-hidden="true" />
            <span className="text-xs tracking-wider text-red-400 font-medium">something broke</span>
          </div>
          <div className="p-4">
            <p className="text-sm text-gray-300 mb-2">
              This page hit an unexpected error and stopped rendering. The rest of your data is safe.
            </p>
            <p className="text-xs text-gray-500 font-mono mb-4 break-all" data-testid="error-boundary-message">
              {error.message || String(error)}
            </p>
            <div className="flex items-center gap-3">
              <button
                onClick={() => window.location.reload()}
                className="px-4 py-1.5 text-xs bg-cyan-600 hover:bg-cyan-500 text-white rounded transition-colors"
              >
                Reload
              </button>
              <button
                onClick={() => { window.location.href = '/'; }}
                className="px-4 py-1.5 text-xs border border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600 rounded transition-colors"
              >
                Back to start
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }
}
