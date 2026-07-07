# Refuse conflicting exposures with Chinese choices

When a new Exposure would collide with an existing Host Entry Name, the tool will refuse to write instead of creating a blocked state or silently renaming the entry. The CLI and Mac UI will show the resolution choices in Simplified Chinese, because conflict handling is a user decision and should be readable without understanding the internal naming model.

Example choices:

- 使用推荐名称，例如 `mattpocock-review`
- 替换现有 entry
- 跳过这次暴露
