/** Storage bounds apply after every delta, including replay snapshots. */
export const TERMINAL_LIMITS = {
  blocks: 128,
  linesPerBlock: 500,
  totalLines: 2000,
  totalCharacters: 512_000,
  lineCharacters: 8192,
  spansPerLine: 128,
  agentBlocks: 128,
  agentCharacters: 8192,
  agentNodes: 128,
  agentDepth: 4,
  altRows: 240,
  metadataCharacters: 2048,
} as const;
