import { useEffect, useRef, useState } from "react";
export function AgentImage({ providerId, agentId, conversationId, itemId, name }: {
  providerId: string; agentId: string; conversationId: string; itemId: string; name: string;
}) {
  const root = useRef<HTMLElement>(null);
  const [visible, setVisible] = useState(false);
  const [image, setImage] = useState("");
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const observer = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting) { setVisible(true); observer.disconnect(); }
    }, { rootMargin: "100px" });
    if (root.current) observer.observe(root.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!visible) return;
    let disposed = false;
    setError(""); setImage("");
    void window.bridge?.agentCall<{ dataUrl: string }>(providerId, "conversations.media", {
      agentId, conversationId, itemId,
    }).then((result) => { if (!disposed) setImage(result.dataUrl); })
      .catch(() => { if (!disposed) setError("Image unavailable. It may have been moved, removed or exceeded the preview limit."); });
    return () => { disposed = true; };
  }, [providerId, agentId, conversationId, itemId, visible, attempt]);
  return <figure ref={root} className="agent-image">
    {image ? <img src={image} alt={name} onError={() => { setImage(""); setError("This image could not be decoded."); }} />
      : <span role="status">{error || "Loading image…"}</span>}
    <figcaption>{name}</figcaption>
    {error && <button onClick={() => setAttempt((value) => value + 1)}>Retry image</button>}
  </figure>;
}
