import { useRef } from "react";
export interface AgentAttachment { name: string; data: string }

export async function readAgentAttachments(files: File[]): Promise<AgentAttachment[]> {
  if (files.some((file) => !file.size || file.size > 1024 * 1024))
    throw Error("Choose non-empty files, no larger than 1 MB each.");
  return Promise.all(files.map((file) => new Promise<AgentAttachment>((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(Error(`Could not read ${file.name}.`));
    reader.onabort = () => reject(Error(`Reading ${file.name} was cancelled.`));
    reader.onload = () => resolve({ name: file.name, data: String(reader.result).split(",")[1] });
    reader.readAsDataURL(file);
  })));
}

export function AgentAttachments({ files, onChange, onAdd, disabled, loading, error }: {
  files: AgentAttachment[];
  onChange: (files: AgentAttachment[]) => void;
  onAdd: (files: File[]) => void;
  disabled: boolean;
  loading: boolean;
  error: string;
}) {
  const picker = useRef<HTMLInputElement>(null);
  return <div className="agent-attachments">
    <div className="agent-attachment-picker">
      <button type="button" disabled={disabled || loading} onClick={() => picker.current?.click()}>Attach files</button>
      <small>Drop or paste · Up to 8 files, 1 MB each</small>
      <input ref={picker} style={{ display: "none" }} type="file" multiple disabled={disabled || loading} aria-label="Attach files"
        onChange={(event) => {
          const selected = Array.from(event.target.files || []);
          event.target.value = "";
          onAdd(selected);
        }} />
    </div>
    {loading && <span role="status">Reading files…</span>}
    {error && <p role="alert">{error}</p>}
    {files.map((file, index) => <span className="agent-attachment" key={`${index}-${file.name}`}>
      <span>{file.name}</span>
      <button type="button" disabled={disabled || loading} aria-label={`Remove ${file.name}`}
        onClick={() => onChange(files.filter((_, i) => i !== index))}>×</button>
    </span>)}
  </div>;
}
