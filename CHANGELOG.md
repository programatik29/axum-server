# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

# Unreleased

None.

# 0.2.3 (14. September 2021)

- Fixed `bind` and `bind_rustls` not working on some types.

# 0.2.2 (6. September 2021)

- Added uri `Scheme` in `Request` extensions.
- Fixed memory leak that happens as connections are accepted.

# 0.2.1 (30. August 2021)

- Fixed `serve_and_record` not recording independently for each connection.

# 0.2.0 (29. August 2021)

- Added `TlsLoader` to reload tls configuration.
- Added `Handle` to provide additional utilities for server.

# 0.1.2 (24. August 2021)

- Fixed an import issue when using `tls-rustls` feature.

# 0.1.0 (23. August 2021)

- Initial release.
