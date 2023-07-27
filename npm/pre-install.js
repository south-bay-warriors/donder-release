#!/usr/bin/env node

const fs = require("fs");
const path = require("path");
const { exec } = require("child_process");

const homeDir =
  process.env.HOME || process.env.USERPROFILE || process.env.HOMEPATH;
const cargoDir = path.join(homeDir, ".cargo");

// check if directory exists
if (!fs.existsSync(cargoDir)) {
  const setCargo = 'PATH="/$HOME/.cargo/bin:$PATH"';
  console.log("Installing deps [cargo].");

  exec(
    `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && ${setCargo}`,
    (error) => {
      if (error) {
        console.log(
          "curl failed! Curl may not be installed on the OS. View https://curl.se/download.html to install."
        );
        console.log(error);
      }
    }
  );
}

const version = process.env.npm_config_version
  ? process.env.npm_config_version
  : require("../package.json").version;

console.log(`Installing and compiling donder-release ${version}...`);
exec(
  `cargo install donder-release --vers ${version}`,
  (error, stdout, stderr) => {
    console.log(stdout);
    if (error || stderr) {
      console.log(error || stderr);
    } else {
      console.log("install finished!");
    }
  }
);
