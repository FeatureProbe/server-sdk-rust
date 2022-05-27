# FeatureProbe Server Side SDK for Rust
[![Top Language](https://img.shields.io/github/languages/top/FeatureProbe/server-sdk-rust)](https://github.com/FeatureProbe/server-sdk-rust/search?l=rust)
[![codecov](https://codecov.io/gh/featureprobe/server-sdk-rust/branch/main/graph/badge.svg?token=TAN3AU4CK2)](https://codecov.io/gh/featureprobe/server-sdk-rust)
[![Github Star](https://img.shields.io/github/stars/FeatureProbe/server-sdk-rust)](https://github.com/FeatureProbe/server-sdk-rust/stargazers)
[![Apache-2.0 license](https://img.shields.io/github/license/FeatureProbe/FeatureProbe)](https://github.com/FeatureProbe/FeatureProbe/blob/main/LICENSE)


Feature Probe is an open source feature management service. This SDK is used to control features in rust programs. This
SDK is designed primarily for use in multi-user systems such as web servers and applications.

## Getting started

In this guide we explain how to use feature toggles in a Rust application using FeatureProbe.

### Step 1. Install the Rust SDK

First, install the FeatureProbe SDK as a dependency in your application.

```shell
cargo install cargo-edit
cargo add feature-probe-server-sdk-rs --allow-prerelease
```

Next, import the FeatureProbe SDK in your application code:

```rust
use feature_probe_server_sdk::{FPConfig, FPUser, FeatureProbe};
```

### Step 2. Create a FeatureProbe instance

After you install and import the SDK, create a single, shared instance of the FeatureProbe sdk.

```rust
fn main() {
    let remote_url = args.host_url + "/api/server/toggles";

    let config = FPConfig {
        remote_url: remote_url.to_owned(),
        server_sdk_key: args.server_sdk_key.to_owned(),
        refresh_interval: Duration::from_secs(5),
        wait_first_resp: true,
    };

    let fp = match FeatureProbe::new(config) {
        Ok(fp) => fp,
        Err(e) => {
            tracing::error!("{:?}", e);
            return;
        }
    };
}
```

### Step 3. Use the feature toggle

You can use sdk to check which variation a particular user will receive for a given feature flag.

```
let user = FPUser::new("user@company.com").with("name", "bob");
let show_feature = fp.bool_value("your.toggle.key", &user, false);

if show_feature {
    # application code to show the feature
} else {
    # the code to run if the feature is off
}
```

## Testing

We have unified integration tests for all our SDKs. Integration test cases are added as submodules for each SDK repo. So
be sure to pull submodules first to get the latest integration tests before running tests.

```shell
git pull --recurse-submodules
cargo test
```

## Contributing
We are working on continue evolving FeatureProbe core, making it flexible and easier to use.
Development of FeatureProbe happens in the open on GitHub, and we are grateful to the
community for contributing bugfixes and improvements.

Please read [CONTRIBUTING](https://github.com/FeatureProbe/featureprobe/blob/master/CONTRIBUTING.md)
for details on our code of conduct, and the process for taking part in improving FeatureProbe.
