// Copyright 2026 houseme
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Binary entry point: parse CLI, run, exit with the mapped code.

use clap::Parser;

use utf8_forge::{cli::Cli, run};

fn main() {
    // Restore SIGPIPE's default disposition: Rust ignores the signal, which
    // turns a closed pipe (`... | head`) into an EPIPE panic inside
    // `println!` (exit 101 with a panic backtrace). The Unix convention is a
    // silent kill on SIGPIPE, which shell pipelines expect.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    std::process::exit(run(cli));
}
