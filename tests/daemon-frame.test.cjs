const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const {
  MAX_FRAME,
  FrameError,
  encode,
  createDecoder,
} = require("../electron/daemon/frame.cjs");

const vectorDir = path.join(
  __dirname,
  "..",
  "crates",
  "sushiai-protocol",
  "vectors",
);
const vectors = fs
  .readdirSync(vectorDir)
  .filter((name) => name.endsWith(".json"))
  .map((name) =>
    JSON.parse(fs.readFileSync(path.join(vectorDir, name), "utf8")),
  );

function expected(frame) {
  if (frame.kind === "J") return { kind: "J", json: frame.json };
  return {
    kind: "B",
    id: frame.id,
    seq: BigInt(frame.seq),
    data: Buffer.from(frame.dataHex, "hex"),
  };
}

test("vector directory is not empty", () => {
  assert.ok(vectors.length >= 4);
});

for (const vector of vectors) {
  test(`vector ${vector.name} decodes and re-encodes to the same bytes`, () => {
    const bytes = Buffer.from(vector.hex, "hex");
    const frames = createDecoder().push(bytes);
    assert.deepEqual(frames, vector.frames.map(expected));
    assert.equal(Buffer.concat(frames.map(encode)).toString("hex"), vector.hex);
  });

  test(`vector ${vector.name} decodes byte by byte`, () => {
    const decoder = createDecoder();
    const frames = [];
    for (const byte of Buffer.from(vector.hex, "hex"))
      frames.push(...decoder.push(Buffer.from([byte])));
    assert.deepEqual(frames, vector.frames.map(expected));
  });
}

test("two frames in one chunk decode in order", () => {
  const a = encode({ kind: "J", json: "{}" });
  const b = encode({ kind: "B", id: "x", seq: 1, data: Buffer.from("hi") });
  const frames = createDecoder().push(Buffer.concat([a, b]));
  assert.equal(frames.length, 2);
  assert.equal(frames[0].kind, "J");
  assert.equal(frames[1].seq, 1n);
});

test("seq above 2^53 round-trips exactly as a BigInt", () => {
  const seq = (1n << 63n) + 12345n;
  const [frame] = createDecoder().push(
    encode({ kind: "B", id: "s", seq, data: Buffer.alloc(0) }),
  );
  assert.equal(frame.seq, seq);
});

test("encode rejects an unsafe Number seq instead of losing precision", () => {
  assert.throws(
    () => encode({ kind: "B", id: "s", seq: 2 ** 53, data: Buffer.alloc(0) }),
    FrameError,
  );
  assert.throws(
    () => encode({ kind: "B", id: "s", seq: -1, data: Buffer.alloc(0) }),
    FrameError,
  );
  assert.throws(
    () => encode({ kind: "B", id: "s", seq: 1n << 64n, data: Buffer.alloc(0) }),
    FrameError,
  );
});

test("oversize frame is rejected on decode and encode", () => {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(MAX_FRAME + 1, 0);
  assert.throws(() => createDecoder().push(header), FrameError);
  assert.throws(
    () => encode({ kind: "J", json: "a".repeat(MAX_FRAME) }),
    FrameError,
  );
});

test("empty, unknown-kind, truncated and invalid UTF-8 frames are rejected", () => {
  assert.throws(() => createDecoder().push(Buffer.from([0, 0, 0, 0])), /empty/);
  assert.throws(
    () => createDecoder().push(Buffer.from([0, 0, 0, 1, 0x5a])),
    /unknown frame kind 0x5a/,
  );
  assert.throws(
    () => createDecoder().push(Buffer.from([0, 0, 0, 3, 0x42, 0, 9])),
    /truncated/,
  );
  assert.throws(
    () => createDecoder().push(Buffer.from([0, 0, 0, 2, 0x4a, 0xff])),
    /UTF-8/,
  );
  assert.throws(() => encode({ kind: "Z" }), FrameError);
});

test("a frame of exactly MAX_FRAME bytes decodes, and MAX_FRAME-1 body bytes encode", () => {
  const body = Buffer.alloc(MAX_FRAME - 1, 0x61);
  const wire = Buffer.alloc(4 + MAX_FRAME);
  wire.writeUInt32BE(MAX_FRAME, 0);
  wire[4] = 0x4a;
  body.copy(wire, 5);
  const [frame] = createDecoder().push(wire);
  assert.equal(frame.json.length, MAX_FRAME - 1);
  const encoded = encode({ kind: "J", json: frame.json });
  assert.equal(encoded.length, 4 + MAX_FRAME);
  assert.ok(encoded.equals(wire));
});
