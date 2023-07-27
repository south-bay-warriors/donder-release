#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const { exec } = require("child_process");

const homeDir =
  process.env.HOME || process.env.USERPROFILE || process.env.HOMEPATH;
const binp = path.join(homeDir, ".cargo", "bin", "donder-release");

if (fs.existsSync(binp)) {
  console.log("Uninstalling donder-release...");
  exec(`cargo uninstall donder-release`, (error, stdout, stderr) => {
    console.log(stdout);
    if (error || stderr) {
      console.log(error || stderr);
    }
  });
} else {
  console.log("donder-release not found skipping!");
}
