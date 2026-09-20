import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

type RenderRequest = {
  expression: string;
  fontSizePt: number;
  color: string;
};

type RenderResult = {
  svg: string;
  width: number;
  height: number;
  cacheKey: string;
};

type TypstStatus = {
  available: boolean;
  version?: string;
  executablePath?: string;
  error?: string;
};

type CopyFormat = "svg" | "png";

type CopyResult = {
  format: CopyFormat;
};

type BackendError = {
  kind?: string;
  message?: string;
  diagnostic?: string;
};

const app = document.querySelector<HTMLDivElement>("#app")!;

app.innerHTML = `
  <main class="app-shell">
    <header class="app-header">
      <div>
        <h1>Typst Equation</h1>
        <p>数式を書いて、そのままスライドへ。</p>
      </div>
      <div id="typst-status" class="runtime-status checking">
        <span class="status-dot"></span><span>Typstを確認中…</span>
      </div>
    </header>

    <section class="toolbar" aria-label="数式の表示設定">
      <label class="control size-control">
        <span>サイズ</span>
        <span class="number-wrap">
          <input id="font-size" type="number" min="8" max="144" value="32" />
          <span>pt</span>
        </span>
      </label>
      <label class="control color-control">
        <span>文字色</span>
        <span class="color-wrap">
          <input id="color" type="color" value="#111827" />
          <code id="color-value">#111827</code>
        </span>
      </label>
      <label class="control format-control">
        <span>コピー形式</span>
        <select id="copy-format" aria-label="コピー形式">
          <option value="svg">SVG（ベクター）</option>
          <option value="png">PNG（互換）</option>
        </select>
      </label>
      <div class="toolbar-spacer"></div>
      <span class="shortcut-hint"><kbd>Ctrl</kbd><span>+</span><kbd>Enter</kbd></span>
      <button id="copy-button" class="copy-button" disabled>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 8V5.8C8 4.81 8.81 4 9.8 4h8.4c.99 0 1.8.81 1.8 1.8v8.4c0 .99-.81 1.8-1.8 1.8H16M5.8 8h8.4c.99 0 1.8.81 1.8 1.8v8.4c0 .99-.81 1.8-1.8 1.8H5.8C4.81 20 4 19.19 4 18.2V9.8C4 8.81 4.81 8 5.8 8Z"/></svg>
        <span>コピー</span>
      </button>
    </section>

    <section class="workspace">
      <div class="panel editor-panel">
        <div class="panel-heading">
          <div><span class="step">1</span><h2>Typstで入力</h2></div>
          <span id="char-count">0文字</span>
        </div>
        <div class="editor-wrap">
          <textarea id="expression" spellcheck="false" aria-label="Typst数式" placeholder="例: integral_0^infinity e^(-x^2) dif x"></textarea>
          <span class="math-marker start">$</span>
          <span class="math-marker end">$</span>
        </div>
        <div id="diagnostic" class="diagnostic" role="alert" hidden></div>
      </div>

      <div class="panel preview-panel">
        <div class="panel-heading">
          <div><span class="step">2</span><h2>プレビュー</h2></div>
          <span id="render-state" class="render-state">入力待ち</span>
        </div>
        <div id="preview" class="preview empty">
          <div class="empty-state">
            <div class="empty-icon">∑</div>
            <p>左側に数式を入力すると<br />ここにプレビューされます</p>
          </div>
        </div>
      </div>
    </section>

    <footer>
      <span>背景は透明です</span>
      <span class="footer-separator">•</span>
      <span>PowerPoint向けSVG + PNG</span>
    </footer>
  </main>
  <div id="toast" class="toast" role="status" aria-live="polite"></div>
`;

const expression = document.querySelector<HTMLTextAreaElement>("#expression")!;
const fontSize = document.querySelector<HTMLInputElement>("#font-size")!;
const color = document.querySelector<HTMLInputElement>("#color")!;
const colorValue = document.querySelector<HTMLElement>("#color-value")!;
const copyFormat = document.querySelector<HTMLSelectElement>("#copy-format")!;
const copyButton = document.querySelector<HTMLButtonElement>("#copy-button")!;
const preview = document.querySelector<HTMLElement>("#preview")!;
const diagnostic = document.querySelector<HTMLElement>("#diagnostic")!;
const renderState = document.querySelector<HTMLElement>("#render-state")!;
const runtimeStatus = document.querySelector<HTMLElement>("#typst-status")!;
const charCount = document.querySelector<HTMLElement>("#char-count")!;
const toast = document.querySelector<HTMLElement>("#toast")!;

let typstAvailable = false;
let debounceTimer: number | undefined;
let latestRequest = 0;
let lastRendered: RenderResult | null = null;
let toastTimer: number | undefined;

function request(): RenderRequest {
  return {
    expression: expression.value,
    fontSizePt: Number(fontSize.value),
    color: color.value.toUpperCase(),
  };
}

function formatError(error: unknown): BackendError {
  if (typeof error === "object" && error !== null) return error as BackendError;
  return { message: String(error) };
}

function showToast(message: string, isError = false) {
  window.clearTimeout(toastTimer);
  toast.textContent = message;
  toast.className = `toast visible${isError ? " error" : ""}`;
  toastTimer = window.setTimeout(() => (toast.className = "toast"), 2600);
}

function setEmptyPreview() {
  preview.className = "preview empty";
  preview.innerHTML = `<div class="empty-state"><div class="empty-icon">∑</div><p>左側に数式を入力すると<br />ここにプレビューされます</p></div>`;
  renderState.textContent = "入力待ち";
  renderState.className = "render-state";
  diagnostic.hidden = true;
  lastRendered = null;
  copyButton.disabled = true;
}

async function render() {
  const current = ++latestRequest;
  const value = expression.value.trim();
  charCount.textContent = `${expression.value.length}文字`;

  if (!value) {
    setEmptyPreview();
    return;
  }
  if (!typstAvailable) return;

  renderState.textContent = "レンダリング中…";
  renderState.className = "render-state loading";
  diagnostic.hidden = true;

  try {
    const result = await invoke<RenderResult>("render_equation", { request: request() });
    if (current !== latestRequest) return;
    lastRendered = result;
    preview.className = "preview";
    preview.innerHTML = result.svg;
    renderState.textContent = "準備完了";
    renderState.className = "render-state ready";
    copyButton.disabled = false;
  } catch (rawError) {
    if (current !== latestRequest) return;
    const error = formatError(rawError);
    renderState.textContent = "エラー";
    renderState.className = "render-state failed";
    diagnostic.hidden = false;
    diagnostic.innerHTML = `<strong>${escapeHtml(error.message ?? "数式をレンダリングできませんでした")}</strong>${error.diagnostic ? `<pre>${escapeHtml(error.diagnostic)}</pre>` : ""}`;
    copyButton.disabled = true;
  }
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>'"]/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;",
  })[character]!);
}

function scheduleRender() {
  window.clearTimeout(debounceTimer);
  debounceTimer = window.setTimeout(render, 300);
}

async function copyEquation() {
  if (!lastRendered || copyButton.disabled) return;
  copyButton.disabled = true;
  copyButton.classList.add("working");
  copyButton.querySelector("span")!.textContent = "コピー中…";
  try {
    const result = await invoke<CopyResult>("copy_equation", {
      request: request(),
      format: copyFormat.value as CopyFormat,
    });
    showToast(`${result.format.toUpperCase()}としてコピーしました。Affinity / PowerPointに貼り付けできます`);
  } catch (rawError) {
    const error = formatError(rawError);
    showToast(error.message ?? "クリップボードへコピーできませんでした", true);
  } finally {
    copyButton.classList.remove("working");
    copyButton.querySelector("span")!.textContent = "コピー";
    copyButton.disabled = !lastRendered;
  }
}

expression.addEventListener("input", scheduleRender);
fontSize.addEventListener("input", scheduleRender);
color.addEventListener("input", () => {
  colorValue.textContent = color.value.toUpperCase();
  scheduleRender();
});
copyButton.addEventListener("click", copyEquation);
document.addEventListener("keydown", (event) => {
  if (event.ctrlKey && event.key === "Enter") {
    event.preventDefault();
    copyEquation();
  }
});

async function initialize() {
  try {
    const status = await invoke<TypstStatus>("get_typst_status");
    typstAvailable = status.available;
    if (status.available) {
      runtimeStatus.className = "runtime-status available";
      runtimeStatus.innerHTML = `<span class="status-dot"></span><span>${escapeHtml(status.version ?? "Typst")}</span>`;
      runtimeStatus.title = status.executablePath ?? "";
      expression.focus();
    } else {
      runtimeStatus.className = "runtime-status unavailable";
      runtimeStatus.innerHTML = `<span class="status-dot"></span><span>Typstが見つかりません</span>`;
      diagnostic.hidden = false;
      diagnostic.innerHTML = `<strong>Typstを利用できません</strong><p>${escapeHtml(status.error ?? "Typstをインストールし、PATHへ追加してからアプリを再起動してください。")}</p>`;
      expression.disabled = true;
    }
  } catch (error) {
    typstAvailable = false;
    runtimeStatus.className = "runtime-status unavailable";
    runtimeStatus.innerHTML = `<span class="status-dot"></span><span>起動エラー</span>`;
    showToast(String(error), true);
  }
}

initialize();
