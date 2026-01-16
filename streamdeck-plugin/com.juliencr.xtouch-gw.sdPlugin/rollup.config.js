import resolve from "@rollup/plugin-node-resolve";
import typescript from "@rollup/plugin-typescript";
import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default {
  input: "src/plugin.ts",
  output: {
    file: "bin/plugin.js",
    format: "es",
    sourcemap: true,
  },
  external: [
    "@elgato/streamdeck",
    "canvas",
    "ws",
    "path",
    "fs",
    "events",
    "url",
  ],
  plugins: [
    resolve({
      preferBuiltins: true,
    }),
    typescript({
      tsconfig: "./tsconfig.json",
      sourceMap: true,
    }),
  ],
};
