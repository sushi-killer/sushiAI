// Pacing of terminal output between main and the renderer: at most
// OUTPUT_CREDIT_BYTES are unacknowledged at once, sent in chunks of at most
// OUTPUT_CHUNK_BYTES (see daemon/terminals.cjs).
const OUTPUT_CREDIT_BYTES = 256 * 1024;
const OUTPUT_CHUNK_BYTES = 32 * 1024;

module.exports = { OUTPUT_CREDIT_BYTES, OUTPUT_CHUNK_BYTES };
