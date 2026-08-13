import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

describe("Library 固定画布", () => {
  it("native main window 在受支持的较小 work area 内不会被裁切", () => {
    const config = JSON.parse(
      readFileSync("src-tauri/tauri.conf.json", "utf8"),
    );
    const mainWindow = config.app.windows.find(
      (window) => window.label === "main",
    );
    const supportedWorkArea = { width: 1000, height: 700 };

    expect(mainWindow).toBeDefined();
    expect(supportedWorkArea.width).toBeGreaterThanOrEqual(
      mainWindow.minWidth,
    );
    expect(supportedWorkArea.height).toBeGreaterThanOrEqual(
      mainWindow.minHeight,
    );
    expect(supportedWorkArea.width).toBeLessThan(mainWindow.width);
    expect(supportedWorkArea.height).toBeLessThan(mainWindow.height);
    expect(mainWindow.preventOverflow).toBe(true);
  });

  it("按 App 提供的显式偏移放置画布，不依赖 Grid 溢出对齐", () => {
    const styles = readFileSync(
      "src/components/library/library.css",
      "utf8",
    );
    const fixedCanvasContainer = styles.match(
      /\.application-theme:has\(\.inventory-library-shell\)\s*\{([^}]*)\}/,
    );
    const fixedCanvas = styles.match(
      /\.application-theme:has\(\.inventory-library-shell\)\s*>\s*\.application-frame\s*\{([^}]*)\}/,
    );

    expect(fixedCanvasContainer).not.toBeNull();
    expect(fixedCanvasContainer?.[1]).toContain("position: relative");
    expect(fixedCanvas).not.toBeNull();
    expect(fixedCanvas?.[1]).toContain("position: absolute");
    expect(fixedCanvas?.[1]).toContain(
      "top: var(--library-offset-y, 0px)",
    );
    expect(fixedCanvas?.[1]).toContain(
      "left: var(--library-offset-x, 0px)",
    );
    expect(fixedCanvas?.[1]).toContain("transform-origin: top left");
  });

  it("紧凑 Layers 露出下一张书脊，提示横向还有内容", () => {
    const styles = readFileSync(
      "src/components/library/library.css",
      "utf8",
    );
    const compactLibrary = styles.match(
      /\.application-theme\[data-library-layout="compact"\] \.layers-library\s*\{([^}]*)\}/,
    );
    const compactCard = styles.match(
      /\.application-theme\[data-library-layout="compact"\][\s\S]*?\.bundle-library\.layers-library[\s\S]*?\.layers-library-card\s*\{([^}]*)\}/,
    );

    expect(compactLibrary).not.toBeNull();
    expect(compactLibrary?.[1]).toContain(
      "grid-template-columns: 132px minmax(0, 1fr)",
    );
    expect(compactCard).not.toBeNull();
    expect(compactCard?.[1]).toContain("flex-basis: 40px");
    expect(compactCard?.[1]).toContain("width: 40px");
  });

  it("Bundle 更新使用清晰的次级动作尺寸，而不是状态胶囊", () => {
    const styles = readFileSync("src/styles.css", "utf8");
    const action = styles.match(/\.bundle-update-action\s*\{([^}]*)\}/);

    expect(action).not.toBeNull();
    expect(action?.[1]).toContain("min-height: 32px");
    expect(action?.[1]).toMatch(/border:\s*1px solid/);
    expect(action?.[1]).not.toContain("border-radius: 999px");
  });
});
