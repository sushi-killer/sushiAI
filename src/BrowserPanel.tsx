import { useEffect, useRef, useState, createElement } from "react";
import {
  ArrowLeft,
  ArrowRight,
  RotateCw,
  Globe,
  ExternalLink,
  LockKeyhole,
} from "lucide-react";
type Webview = HTMLElement & {
  reload(): void;
  goBack(): void;
  goForward(): void;
  canGoBack(): boolean;
  canGoForward(): boolean;
  getURL(): string;
};
export function BrowserPanel({
  url,
  onNavigate,
  endpoint,
  sourceFile,
}: {
  url?: string;
  onNavigate(url: string): void;
  endpoint?: string;
  sourceFile?: { root: string; path: string; endpoint?: string };
}) {
  const [draft, setDraft] = useState(url || ""),
    [error, setError] = useState("");
  const view = useRef<Webview>(null);
  const [previewURL, setPreviewURL] = useState("");
  const actualURL = previewURL;
  useEffect(() => {
    let disposed = false;
    setPreviewURL("");
    setError("");
    const resolve = async () => {
      if (sourceFile && window.bridge)
        return window.bridge.projectPreview(
          sourceFile.endpoint,
          sourceFile.root,
          sourceFile.path,
        );
      if (!url) return "";
      const parsed = new URL(url);
      if (
        endpoint?.startsWith("ssh:") &&
        ["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname)
      )
        return window.bridge!.connectionsForward(endpoint, url);
      return url;
    };
    resolve()
      .then((value) => {
        if (!disposed) {
          setPreviewURL(value);
          setDraft(sourceFile?.path || url || "");
        }
      })
      .catch((e) => {
        if (!disposed) setError(e.message);
      });
    return () => {
      disposed = true;
    };
  }, [sourceFile?.root, sourceFile?.path, sourceFile?.endpoint, url, endpoint]);
  useEffect(() => {
    const current = view.current;
    if (!current) return;
    const fail = (event: Event) => {
      const e = event as Event & {
        errorCode: number;
        errorDescription: string;
        isMainFrame: boolean;
      };
      if (e.errorCode !== -3 && e.isMainFrame)
        setError(
          e.errorDescription ||
            "Could not load this page. Check that your development server is running.",
        );
    };
    const navigate = () => {
      setDraft(
        sourceFile?.path ||
          (endpoint?.startsWith("ssh:")
            ? url || current.getURL()
            : current.getURL()),
      );
      setError("");
    };
    current.addEventListener("did-fail-load", fail);
    current.addEventListener("did-navigate", navigate);
    return () => {
      current.removeEventListener("did-fail-load", fail);
      current.removeEventListener("did-navigate", navigate);
    };
  }, [actualURL]);
  const go = async (value: string) => {
    try {
      const normalized = new URL(
        value.includes("://") ? value : `http://${value}`,
      );
      if (!["http:", "https:"].includes(normalized.protocol)) throw new Error();
      setError("");
      setDraft(normalized.href);
      onNavigate(normalized.href);
    } catch (e) {
      setError(
        e instanceof Error
          ? e.message
          : "Enter an http:// or https:// address.",
      );
    }
  };
  return (
    <div className="browser">
      <div className="browser-toolbar">
        <button
          aria-label="Back"
          onClick={() => {
            if (view.current?.canGoBack()) view.current.goBack();
          }}
        >
          <ArrowLeft />
        </button>
        <button
          aria-label="Forward"
          onClick={() => {
            if (view.current?.canGoForward()) view.current.goForward();
          }}
        >
          <ArrowRight />
        </button>
        <button
          aria-label="Reload page"
          onClick={() => {
            setError("");
            view.current?.reload();
          }}
        >
          <RotateCw />
        </button>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            go(draft);
          }}
        >
          <Globe size={11} />
          <input
            aria-label="Browser address"
            placeholder="localhost:3000"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
          <LockKeyhole size={10} />
        </form>
      </div>
      {actualURL ? (
        <div className="browser-content">
          {window.bridge ? (
            createElement("webview", {
              ref: view,
              src: actualURL,
              partition: "persist:workspace-browser",
              className: "webview",
            })
          ) : (
            <iframe
              title="Browser preview"
              src={actualURL}
              sandbox="allow-scripts allow-forms allow-same-origin"
            />
          )}
          {error && (
            <div className="browser-error">
              <Globe size={25} />
              <strong>Unable to open this page</strong>
              <p>{error}</p>
              <button
                onClick={() => {
                  setError("");
                  view.current?.reload();
                }}
              >
                Try again
              </button>
            </div>
          )}
        </div>
      ) : (
        <div className="browser-start">
          <div className="preview-top">
            <span />
            <span />
          </div>
          <div className="preview-cards">
            <div />
            <div className="blue-card">
              <Globe size={24} />
            </div>
            <div />
          </div>
          <div className="preview-line" />
          <div className="preview-caption">
            <span>Your app, right here.</span>
            <button onClick={() => go("http://localhost:3000")}>
              Open localhost:3000 <ExternalLink size={11} />
            </button>
          </div>
          {error && <p className="error-text">{error}</p>}
        </div>
      )}
    </div>
  );
}
