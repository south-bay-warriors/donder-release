#!/usr/bin/env node

const { spawn } = require("child_process");

const [, , ...args] = process.argv;

const child = spawn("donder-release", args, { stdio: "inherit" });

child.on("close", (code) => {
  process.exit(code ?? 1);
});

process.on("SIGTERM", () => {
  child.kill("SIGTERM");
});
