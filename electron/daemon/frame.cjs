"use strict";

// Sans-IO frame codec. Mirrors crates/sushiai-protocol/src/frame.rs:
// u32 BE length of (kind + payload), u8 kind, payload.
//   'J' payload = UTF-8 JSON-RPC message
//   'B' payload = u16 BE id length, id bytes, u64 BE seq, raw bytes
// A decoded `seq` is always a BigInt so offsets above 2^53 stay exact.
// encode() accepts a BigInt or a safe-integer Number and rejects anything else.

const MAX_FRAME = 16 * 1024 * 1024;
const KIND_JSON = 0x4a;
const KIND_OUTPUT = 0x42;
const MAX_U64 = (1n << 64n) - 1n;

class FrameError extends Error {
  constructor(message) {
    super(message);
    this.name = "FrameError";
  }
}

function toSeq(value) {
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new FrameError(
        `seq ${value} is not a safe non-negative integer; use a BigInt`,
      );
    }
    return BigInt(value);
  }
  if (typeof value === "bigint" && value >= 0n && value <= MAX_U64)
    return value;
  throw new FrameError("seq must be a u64");
}

function encode(frame) {
  let kind;
  let body;
  if (frame.kind === "J") {
    kind = KIND_JSON;
    body = Buffer.from(frame.json, "utf8");
  } else if (frame.kind === "B") {
    const id = Buffer.from(frame.id, "utf8");
    if (id.length > 0xffff) throw new FrameError("session id exceeds u16");
    const data = Buffer.from(frame.data || []);
    body = Buffer.alloc(10 + id.length + data.length);
    body.writeUInt16BE(id.length, 0);
    id.copy(body, 2);
    body.writeBigUInt64BE(toSeq(frame.seq), 2 + id.length);
    data.copy(body, 10 + id.length);
    kind = KIND_OUTPUT;
  } else {
    throw new FrameError(`unknown frame kind ${frame.kind}`);
  }
  if (body.length + 1 > MAX_FRAME)
    throw new FrameError(
      `frame of ${body.length + 1} bytes exceeds the ${MAX_FRAME} byte limit`,
    );
  const out = Buffer.alloc(5 + body.length);
  out.writeUInt32BE(body.length + 1, 0);
  out[4] = kind;
  body.copy(out, 5);
  return out;
}

function utf8(bytes) {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw new FrameError("invalid UTF-8");
  }
}

function parse(body) {
  const kind = body[0];
  const payload = body.subarray(1);
  if (kind === KIND_JSON) return { kind: "J", json: utf8(payload) };
  if (kind !== KIND_OUTPUT)
    throw new FrameError(
      `unknown frame kind 0x${kind.toString(16).padStart(2, "0")}`,
    );
  if (payload.length < 2) throw new FrameError("truncated output frame");
  const idLen = payload.readUInt16BE(0);
  if (payload.length < 2 + idLen + 8)
    throw new FrameError("truncated output frame");
  return {
    kind: "B",
    id: utf8(payload.subarray(2, 2 + idLen)),
    seq: payload.readBigUInt64BE(2 + idLen),
    data: Buffer.from(payload.subarray(10 + idLen)),
  };
}

// push(chunk) returns the frames completed so far. A thrown FrameError is
// fatal for the stream: the caller must drop the connection. Chunks are kept
// in a list and joined only when a whole frame is available.
function createDecoder() {
  const chunks = [];
  let total = 0;

  function take(n) {
    const parts = [];
    let need = n;
    while (need > 0) {
      const head = chunks[0];
      if (head.length <= need) {
        parts.push(head);
        chunks.shift();
        need -= head.length;
      } else {
        parts.push(head.subarray(0, need));
        chunks[0] = head.subarray(need);
        need = 0;
      }
    }
    total -= n;
    return parts.length === 1 ? parts[0] : Buffer.concat(parts);
  }

  function peekLength() {
    if (chunks[0].length >= 4) return chunks[0].readUInt32BE(0);
    const header = Buffer.concat(chunks.slice(0, 4)).subarray(0, 4);
    return header.readUInt32BE(0);
  }

  return {
    push(chunk) {
      if (chunk.length) {
        chunks.push(chunk);
        total += chunk.length;
      }
      const frames = [];
      while (total >= 4) {
        const len = peekLength();
        if (len > MAX_FRAME)
          throw new FrameError(
            `frame of ${len} bytes exceeds the ${MAX_FRAME} byte limit`,
          );
        if (len === 0) throw new FrameError("empty frame");
        if (total - 4 < len) break;
        frames.push(parse(take(4 + len).subarray(4)));
      }
      return frames;
    },
  };
}

module.exports = { MAX_FRAME, FrameError, encode, createDecoder };
