# TOML

TOML support is available through the [TOML extension](https://github.com/zed-industries/zed/tree/main/extensions/toml).

- Tree-sitter: [tree-sitter/tree-sitter-toml](https://github.com/tree-sitter/tree-sitter-toml)
- Language Server: [tamasfe/taplo](https://github.com/tamasfe/taplo)

## Configuration

You can control the behavior of the Taplo TOML language server by adding a `.taplo.toml` file to the root of your project. See the [Taplo Configuration File](https://taplo.tamasfe.dev/configuration/file.html#configuration-file) and [Taplo Formatter Options](https://taplo.tamasfe.dev/configuration/formatter-options.html) documentation for more.

```toml
# .taplo.toml
include = ["Cargo.toml", "some_directory/**/*.toml"]
# exclude = ["Cargo.toml"]

[formatting]
align_entries = true
reorder_keys = true
```

Alternatively, you can pass taplo configuration options via [Zed LSP Settings](../configuring-zed.md#lsp)

```json
  "lsp": {
    "taplo": {
      "settings": {
        "array_auto_collapse": false
      }
    }
  }
```
