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

//! Behavior when the crate is built without the `remote` feature: `--url`
//! must fail with the documented fatal error instead of panicking or
//! producing a confusing transport error.

#![cfg(not(feature = "remote"))]

use utf8_forge::cli::{Commands, ConvertArgs, TreeArgs};
use utf8_forge::run;

#[test]
fn url_without_remote_feature_is_a_fatal_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("never.bin");
    let args = ConvertArgs {
        common: TreeArgs {
            root: dir.path().join("tree"),
            ..Default::default()
        },
        url: Some("https://example.com/a.cpp".into()),
        output: Some(out),
        ..Default::default()
    };
    assert_eq!(run(Commands::Convert(args).into()), 2);
}
