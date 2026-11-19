// Metro is scoped to mobile/ so the bundler can never pick up
// ../../server or ../../dragent sources, even on accidental relative imports.
const { getDefaultConfig } = require("expo/metro-config");

const config = getDefaultConfig(__dirname);

// Only watch this folder: no symlinks or watch roots leak into the repo root.
config.watchFolders = [__dirname];

module.exports = config;
