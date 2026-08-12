// Vite 在构建时把本地 PNG 解析为静态资源 URL。
declare module "*.png" {
  const source: string;
  export default source;
}
