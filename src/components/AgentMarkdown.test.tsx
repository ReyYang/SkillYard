import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { AgentMarkdown } from "./AgentMarkdown";

describe("AgentMarkdown", () => {
  it("流式状态会整理未闭合 Markdown，而不是显示原始标记", () => {
    render(
      <AgentMarkdown streaming onOpenExternalUrl={vi.fn()}>
        {"## 使用方式\n\n- 先扫描\n- 再选择 **Bundle"}
      </AgentMarkdown>,
    );

    expect(
      screen.getByRole("heading", { name: "使用方式" }),
    ).toBeInTheDocument();
    expect(screen.getByText("先扫描")).toBeInTheDocument();
    expect(
      screen
        .getAllByRole("listitem")
        .find((item) => item.textContent === "再选择 Bundle"),
    ).toBeInTheDocument();
  });

  it("忽略 Raw HTML、远程图片和危险协议，并由受控命令打开安全链接", async () => {
    const user = userEvent.setup();
    const onOpenExternalUrl = vi.fn();
    const { container } = render(
      <AgentMarkdown
        streaming={false}
        onOpenExternalUrl={onOpenExternalUrl}
      >
        {
          '<script>window.bad = true</script>\n![远程图](https://example.com/a.png)\n[危险](javascript:alert(1))\n[官方来源](https://example.com/skill)'
        }
      </AgentMarkdown>,
    );

    expect(container.querySelector("script")).not.toBeInTheDocument();
    expect(container.querySelector("img")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "危险" }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "官方来源" }));
    expect(onOpenExternalUrl).toHaveBeenCalledWith(
      "https://example.com/skill",
    );
  });

  it("代码块保持为不可执行的阅读内容且不出现复制或下载控制", () => {
    const { container } = render(
      <AgentMarkdown streaming={false} onOpenExternalUrl={vi.fn()}>
        {"```sh\nrm -rf fixture\n```"}
      </AgentMarkdown>,
    );

    expect(container.querySelector("pre code")).toHaveTextContent(
      "rm -rf fixture",
    );
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("宽表格保留显式横向滚动容器", () => {
    const { container } = render(
      <AgentMarkdown streaming={false} onOpenExternalUrl={vi.fn()}>
        {
          "| 名称 | 很长的来源路径 |\n| --- | --- |\n| Demo | /Users/demo/.agents/skills/a-very-long-skill-name/SKILL.md |"
        }
      </AgentMarkdown>,
    );

    const table = container.querySelector("table");
    const tableWrapper = container.querySelector(
      '[data-streamdown="table-wrapper"]',
    );

    expect(table).toBeInTheDocument();
    expect(tableWrapper).toBeInTheDocument();
    expect(table?.parentElement).toBe(
      tableWrapper?.querySelector(":scope > div:last-child"),
    );
  });
});
