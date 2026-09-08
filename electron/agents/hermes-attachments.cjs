const { text } = require("./registry.cjs");
const MAX_BYTES = 1024 * 1024;

function validateAttachments(value) {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 8) throw Error("Attach up to 8 files.");
  return value.map((file) => {
    const name = text(file?.name, "attachment name", 240);
    if (/[\\/\r\n]/.test(name)) throw Error("Invalid attachment name.");
    const data = file.data;
    if (typeof data !== "string" || data.length > Math.ceil(MAX_BYTES / 3) * 4 ||
        !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(data))
      throw Error("Invalid attachment data (maximum 1 MB per file).");
    const bytes = Buffer.from(data, "base64");
    if (!bytes.length || bytes.length > MAX_BYTES || bytes.toString("base64") !== data)
      throw Error("Attachments must contain between 1 byte and 1 MB.");
    return { name, data, image: /\.(png|jpe?g|gif|webp|bmp)$/i.test(name) };
  });
}

async function stageAttachments(rpc, runtime, files) {
  const refs = [], images = [], media = [];
  const detach = async () => {
    const results = await Promise.allSettled(images.map((path) =>
      rpc("image.detach", { session_id: runtime, path })));
    if (results.some((r) => r.status === "rejected"))
      throw Error("An image could not be detached. Reopen this conversation before retrying.");
  };
  try {
    for (const file of files) {
      const result = await rpc(file.image ? "image.attach_bytes" : "file.attach", {
        session_id: runtime,
        ...(file.image ? { filename: file.name, content_base64: file.data }
          : { name: file.name, data_url: `data:application/octet-stream;base64,${file.data}` }),
      });
      if (result.attached !== true || typeof result.path !== "string")
        throw Error(`Could not attach ${file.name}.`);
      if (file.image) { images.push(result.path); media.push({path:result.path,name:file.name}); }
      else if (typeof result.ref_text !== "string" || !result.ref_text.startsWith("@file:"))
        throw Error(`Hermes did not return a file reference for ${file.name}.`);
      refs.push(file.image ? `[Attached image: ${file.name}]` : result.ref_text);
    }
    return { text: refs.join("\n"), media, detach };
  } catch (error) {
    await detach();
    throw error;
  }
}
module.exports = { validateAttachments, stageAttachments };
