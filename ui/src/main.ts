import { invoke } from '@tauri-apps/api/core';
import './styles.css';

type RecoveryKind =
  | 'None'
  | 'RetryLater'
  | 'RetrySoon'
  | 'ManualOnly'
  | 'Reauth'
  | 'SwitchModel'
  | 'ToolRetryWithDifferentPath'
  | 'SafetyBlocked';

interface ThreadSummary {
  id: string;
  title: string;
  cwd: string;
  updated_at: number;
  rollout_path: string;
}

interface LogEvent {
  ts: number;
  level: string;
  target: string;
  thread_id: string | null;
  body: string;
}

interface RecoveryDecision {
  kind: RecoveryKind;
  auto_allowed: boolean;
  delay_seconds: number;
  label: string;
  reason: string;
}

interface ThreadRecovery {
  thread: ThreadSummary;
  event: LogEvent;
  decision: RecoveryDecision;
}

interface ThreadFeedback {
  thread_id: string;
  title: string;
  timestamp: string | null;
  text: string;
}

interface SentinelStatus {
  checked_at: string;
  codex_running: boolean;
  recent_threads: ThreadSummary[];
  latest_turn_error: LogEvent | null;
  latest_model_error: LogEvent | null;
  latest_stream_retry: LogEvent | null;
  latest_tool_error: LogEvent | null;
  recovery: RecoveryDecision;
}

interface ConfigSummary {
  config_path: string;
  config_dir: string;
  telegram_enabled: boolean;
  telegram_token_configured: boolean;
  allowed_user_count: number;
  allowed_chat_count: number;
  watch_enabled: boolean;
  poll_interval_seconds: number;
  auto_recover: boolean;
  max_recoveries_per_thread: number;
  cooldown_seconds: number;
  continue_prompt: string;
  tool_failure_prompt: string;
  latest_feedback_enabled: boolean;
  completion_notifications_enabled: boolean;
  record_normal_completion_events: boolean;
  hook_event_max_lines: number;
  hook_cooldown_max_lines: number;
  control_queue_max_lines: number;
  log_max_bytes: number;
}

interface HookStatus {
  feature_enabled: boolean;
  config_path: string;
  hooks_path: string;
  hooks_file_exists: boolean;
  stop_installed: boolean;
  installed_app_command: boolean;
  current_executable: string;
  installed_commands: string[];
  recent_events: HookEvent[];
  notes: string[];
}

interface HookEvent {
  ts: string;
  event: string | null;
  action: string;
  event_key: string;
  source: string;
  session_id: string | null;
  turn_id: string | null;
  delay_seconds: number;
  decision_label: string;
  decision_kind: string;
  body: string;
}

interface DashboardPayload {
  status: SentinelStatus;
  config: ConfigSummary;
  hooks: HookStatus;
  telegram: TelegramSettings;
  desktop_control: DesktopControlStatus;
  recoverable_threads: ThreadRecovery[];
  active_feedback: ThreadFeedback | null;
}

interface ContinueResult {
  thread_id: string;
  turn_id: string;
}

interface ControlResponse {
  request_id: string;
  completed_at: number;
  ok: boolean;
  message: string;
  thread_id: string | null;
  turn_id: string | null;
  data: unknown | null;
}

interface TelegramSettings {
  enabled: boolean;
  bot_token_masked: string;
  token_configured: boolean;
  allowed_user_ids: string;
  allowed_chat_ids: string;
  pairing_enabled: boolean;
  pairing_code: string;
  daemon_running: boolean;
  config_path: string;
  daemon_log_path: string;
}

interface DesktopControlStatus {
  mode: string;
  accessibility_granted: boolean;
  screen_recording_granted: boolean;
  notes: string[];
}

interface TelegramDraft {
  enabled: boolean;
  bot_token: string;
  allowed_user_ids: string;
  allowed_chat_ids: string;
  pairing_enabled: boolean;
  pairing_code: string;
}

interface RuntimeDraft {
  watch_enabled: boolean;
  poll_interval_seconds: number;
  auto_recover: boolean;
  max_recoveries_per_thread: number;
  cooldown_seconds: number;
  continue_prompt: string;
  tool_failure_prompt: string;
  latest_feedback_enabled: boolean;
  completion_notifications_enabled: boolean;
  record_normal_completion_events: boolean;
  hook_event_max_lines: number;
  hook_cooldown_max_lines: number;
  control_queue_max_lines: number;
  log_max_bytes: number;
}

interface TelegramPairResult {
  user_id: number | null;
  chat_id: number;
  chat_type: string;
  chat_label: string;
  user_label: string;
  update_id: number;
  dashboard: DashboardPayload;
}

interface TelegramBotCheck {
  id: number;
  username: string;
  first_name: string;
}

interface DaemonStartResult {
  already_running: boolean;
  pid: number | null;
  log_path: string;
}

interface ViewState {
  payload: DashboardPayload | null;
  telegramDraft: TelegramDraft | null;
  runtimeDraft: RuntimeDraft | null;
  pairCode: string;
  pairing: boolean;
  newThreadPrompt: string;
  newThreadPath: string;
  loading: boolean;
  error: string | null;
  notice: string | null;
}

const app = document.querySelector<HTMLDivElement>('#app');
const isTauriRuntime = Reflect.has(window, '__TAURI_INTERNALS__') || Reflect.has(window, '__TAURI__');
const useMock = !isTauriRuntime || new URLSearchParams(location.search).has('mock');
const state: ViewState = {
  payload: null,
  telegramDraft: null,
  runtimeDraft: null,
  pairCode: createPairCode(),
  pairing: false,
  newThreadPrompt: '',
  newThreadPath: '',
  loading: true,
  error: null,
  notice: null,
};
let dashboardInFlight = false;
const DASHBOARD_REFRESH_MS = 20_000;

if (!app) {
  throw new Error('missing app root');
}

loadDashboard();
window.setInterval(() => {
  if (!state.loading && !dashboardInFlight && !hasEditableFocus()) {
    void loadDashboard(false);
  }
}, DASHBOARD_REFRESH_MS);

async function loadDashboard(showSpinner = true) {
  if (dashboardInFlight) {
    return;
  }
  dashboardInFlight = true;
  if (showSpinner) {
    state.loading = true;
    state.error = null;
    render();
  }
  try {
    state.payload = useMock ? mockDashboard() : await invoke<DashboardPayload>('dashboard');
    if (!state.telegramDraft) {
      state.telegramDraft = draftFromTelegram(state.payload.telegram);
    }
    if (!state.runtimeDraft) {
      state.runtimeDraft = draftFromConfig(state.payload.config);
    }
    if (!state.newThreadPath) {
      state.newThreadPath = state.payload.status.recent_threads[0]?.cwd ?? '';
    }
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    dashboardInFlight = false;
    if (showSpinner) {
      state.loading = false;
    }
    render();
  }
}

async function saveTelegramSettings() {
  const input = collectTelegramInput();
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.payload = mockDashboard({ telegramEnabled: input.enabled, telegramDaemon: false });
      state.telegramDraft = { ...input, bot_token: '' };
      state.notice = 'Mock 模式：Telegram 配置已保存。';
      return;
    }
    state.payload = await invoke<DashboardPayload>('save_telegram_settings', { input });
    state.telegramDraft = draftFromTelegram(state.payload.telegram);
    state.notice = 'Telegram 配置已保存。';
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function testTelegramBot() {
  const input = collectTelegramInput();
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：机器人 @codex_sentinel_demo_bot 可用。';
      return;
    }
    const result = await invoke<TelegramBotCheck>('test_telegram_bot', { input });
    const username = result.username ? `@${result.username}` : result.first_name;
    state.notice = `机器人验证通过：${username} / id ${result.id}`;
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function pairTelegramBot() {
  const input = collectTelegramInput();
  const code = input.pairing_code || state.pairCode;
  if (!input.bot_token && !state.payload?.telegram.token_configured) {
    state.error = '先填 Bot Token，再开始配对。';
    render();
    return;
  }
  state.loading = true;
  state.pairing = true;
  state.error = null;
  state.notice = `等待 Telegram 配对消息：/pair ${code}`;
  render();
  try {
    if (useMock) {
      state.payload = mockDashboard({ telegramEnabled: true, telegramDaemon: false, telegramPaired: true });
      state.telegramDraft = draftFromTelegram(state.payload.telegram);
      state.notice = `Mock 模式：已配对 123456789 / 123456789。`;
      state.pairCode = createPairCode();
      return;
    }
    const result = await invoke<TelegramPairResult>('pair_telegram_bot', { input, code });
    state.payload = result.dashboard;
    state.telegramDraft = draftFromTelegram(result.dashboard.telegram);
    state.notice = `配对成功：${result.user_label} -> ${result.chat_label}`;
    state.pairCode = createPairCode();
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    state.pairing = false;
    render();
  }
}

async function copyPairCommand() {
  const draft = state.telegramDraft;
  const command = `/pair ${draft?.pairing_code || state.pairCode}`;
  try {
    await navigator.clipboard.writeText(command);
    state.notice = `已复制：${command}`;
  } catch {
    state.notice = `配对命令：${command}`;
  }
  render();
}

function rotatePairCode() {
  state.pairCode = createPairCode();
  state.telegramDraft = {
    ...(state.telegramDraft ?? {
      enabled: true,
      bot_token: '',
      allowed_user_ids: '',
      allowed_chat_ids: '',
      pairing_enabled: true,
      pairing_code: '',
    }),
    pairing_code: state.pairCode,
  };
  state.notice = `新配对码：${state.pairCode}`;
  render();
}

async function sendTelegramTestMessage() {
  const confirmed = window.confirm('这会向 allowed_chat_ids 里的 Telegram 会话发送一条测试消息。确认发送？');
  if (!confirmed) {
    return;
  }
  const input = collectTelegramInput();
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：测试消息已模拟发送。';
      return;
    }
    state.notice = await invoke<string>('send_telegram_test_message', { input });
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function startTelegramDaemon() {
  const input = collectTelegramInput();
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.payload = mockDashboard({ telegramEnabled: true, telegramDaemon: true });
      state.notice = 'Mock 模式：Telegram 后台已启动。';
      return;
    }
    state.payload = await invoke<DashboardPayload>('save_telegram_settings', { input });
    const result = await invoke<DaemonStartResult>('start_telegram_daemon');
    state.payload = await invoke<DashboardPayload>('dashboard');
    state.telegramDraft = draftFromTelegram(state.payload.telegram);
    state.notice = result.already_running
      ? `Telegram 后台已经在运行。日志：${result.log_path}`
      : `Telegram 后台已启动，pid ${result.pid}。日志：${result.log_path}`;
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function installHooks() {
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：Hooks 安装流程已模拟。';
      state.payload = mockDashboard({ hooksReady: true });
      return;
    }
    const result = await invoke<{ changed_files: string[]; backup_files: string[] }>('install_hooks');
    state.notice = result.changed_files.length
      ? `Hooks 已安装，已更新 ${result.changed_files.length} 个文件。`
      : 'Hooks 已经是最新配置。';
    state.payload = await invoke<DashboardPayload>('dashboard');
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function continueThread() {
  const target = continueTarget(state.payload);
  if (!target) {
    state.error = '没有找到最近的 Codex 线程，无法发送继续指令。';
    state.notice = null;
    render();
    return;
  }
  state.loading = true;
  state.error = null;
  state.notice = `正在打开 Codex APP，并在可见输入框发送到「${target.thread.title || target.thread.id}」。`;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：已模拟发送继续指令。';
      return;
    }
    const result = await invoke<ContinueResult>('continue_current_thread', { threadId: target.thread.id });
    state.notice = `已在 Codex APP 可见窗口发送继续指令：${result.thread_id} / ${result.turn_id}`;
    state.payload = await invoke<DashboardPayload>('dashboard');
    window.setTimeout(() => void loadDashboard(false), 2_000);
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function submitThreadInstruction(thread: ThreadSummary) {
  const input = app.querySelector<HTMLTextAreaElement>(`[data-thread-prompt="${cssEscape(thread.id)}"]`);
  const prompt = input?.value.trim() ?? '';
  if (!prompt) {
    state.error = '追加指令为空。';
    state.notice = null;
    render();
    return;
  }
  state.loading = true;
  state.error = null;
  state.notice = `正在发送追加指令到「${thread.title || thread.id}」。`;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：已模拟追加指令。';
      return;
    }
    const response = await invoke<ControlResponse>('submit_thread_instruction', {
      threadId: thread.id,
      prompt,
    });
    state.notice = `${response.message} ${response.turn_id ?? ''}`.trim();
    state.payload = await invoke<DashboardPayload>('dashboard');
    window.setTimeout(() => void loadDashboard(false), 2_000);
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function startNewThreadFromPanel() {
  const prompt = app.querySelector<HTMLTextAreaElement>('#new-thread-prompt')?.value.trim() ?? state.newThreadPrompt.trim();
  const path = app.querySelector<HTMLInputElement>('#new-thread-path')?.value.trim() ?? state.newThreadPath.trim();
  if (!prompt) {
    state.error = '新线程指令为空。';
    state.notice = null;
    render();
    return;
  }
  state.newThreadPrompt = prompt;
  state.newThreadPath = path;
  state.loading = true;
  state.error = null;
  state.notice = '正在 Codex APP 内创建新线程。';
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：已模拟创建新线程。';
      state.newThreadPrompt = '';
      return;
    }
    const response = await invoke<ControlResponse>('start_new_thread', { prompt, path });
    state.notice = `${response.message} ${response.thread_id ?? ''}`.trim();
    state.newThreadPrompt = '';
    state.payload = await invoke<DashboardPayload>('dashboard');
    window.setTimeout(() => void loadDashboard(false), 2_000);
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function archiveThreadFromPanel(thread: ThreadSummary) {
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：线程已模拟删除。';
      return;
    }
    state.payload = await invoke<DashboardPayload>('archive_thread', { threadId: thread.id });
    state.notice = '线程已删除并从最近列表移除。';
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function clearArchivedThreadsFromPanel() {
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：已模拟清除归档线程。';
      return;
    }
    state.payload = await invoke<DashboardPayload>('clear_archived_threads');
    state.notice = '已清除归档线程。';
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function saveRuntimeSettings() {
  const input = collectRuntimeInput();
  state.runtimeDraft = input;
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.payload = mockDashboard({ autoRecover: input.auto_recover });
      state.notice = 'Mock 模式：运行参数已保存。';
      return;
    }
    state.payload = await invoke<DashboardPayload>('save_runtime_settings', { input });
    state.runtimeDraft = draftFromConfig(state.payload.config);
    state.notice = '运行参数已保存。';
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

function continueTarget(payload: DashboardPayload | null) {
  const recoverable = payload?.recoverable_threads?.[0];
  if (recoverable) {
    return { thread: recoverable.thread, source: 'recoverable' as const };
  }
  const recent = payload?.status.recent_threads[0];
  if (recent) {
    return { thread: recent, source: 'recent' as const };
  }
  return null;
}

async function toggleAutoRecover(enabled: boolean) {
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    state.payload = useMock
      ? mockDashboard({ autoRecover: enabled })
      : await invoke<DashboardPayload>('set_auto_recover', { enabled });
    state.notice = enabled ? '可见自动恢复已开启。' : '可见自动恢复已关闭。';
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function revealConfigDir() {
  try {
    if (useMock) {
      state.notice = 'Mock 模式：配置目录按钮可用。';
      render();
      return;
    }
    await invoke('reveal_config_dir');
  } catch (error) {
    state.error = stringifyError(error);
    render();
  }
}

async function openDesktopPermissions() {
  state.loading = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    if (useMock) {
      state.notice = 'Mock 模式：已打开系统权限设置。';
      return;
    }
    state.payload = await invoke<DashboardPayload>('open_desktop_permissions');
    state.notice = '已打开系统权限设置。授权辅助功能后回到这里刷新状态。';
  } catch (error) {
    state.error = stringifyError(error);
  } finally {
    state.loading = false;
    render();
  }
}

function render() {
  const payload = state.payload;
  if (!payload) {
    app.innerHTML = renderShell(`
      <section class="loading-panel">
        <div class="brand-mark">${logoMark()}</div>
        <h1>连接 Codex Sentinel</h1>
        <p>${state.error ? escapeHtml(state.error) : '正在读取本机 Codex 状态。'}</p>
      </section>
    `);
    return;
  }

  const { status, config, hooks } = payload;
  const telegram = payload.telegram;
  const desktopControl = payload.desktop_control;
  const telegramDraft = state.telegramDraft ?? draftFromTelegram(telegram);
  const runtimeDraft = state.runtimeDraft ?? draftFromConfig(config);
  const activeThread = status.recent_threads[0] ?? null;
  const recoverableThreads = payload.recoverable_threads ?? [];
  const primaryRecovery = recoverableThreads[0];
  const displayedDecision = primaryRecovery?.decision ?? status.recovery;
  const target = continueTarget(payload);
  const hooksReady = hooks.feature_enabled && hooks.stop_installed;
  const hookNotes = actionableHookNotes(hooks.notes);
  const recoveryTone = toneForRecovery(displayedDecision.kind);

  app.innerHTML = renderShell(`
    <header class="topbar">
      <div class="brand">
        <div class="brand-mark">${logoMark()}</div>
        <div>
          <strong>Codex Sentinel</strong>
          <span>Codex APP 可见控制台</span>
        </div>
      </div>
      <div class="top-actions">
        <button class="secondary" data-action="refresh">${state.loading ? '处理中' : '刷新'}</button>
        <button class="secondary" data-action="install-hooks">${hooksReady ? '修复 Stop Hook' : '安装 Stop Hook'}</button>
        <button class="secondary danger-soft" data-action="clear-archived">清除归档</button>
        <button class="${primaryRecovery ? 'danger' : 'secondary'}" data-action="continue">
          ${primaryRecovery ? '可见恢复' : '可见继续'}
        </button>
      </div>
    </header>

    ${renderNotice()}

    <section class="overview-grid">
      ${healthCard('Codex APP', status.codex_running, status.codex_running ? '正在运行' : '未发现进程')}
      ${healthCard('可见输入', desktopControl.accessibility_granted, desktopControl.accessibility_granted ? '已授权，可直接操作 Codex 窗口' : '需要辅助功能权限')}
      ${healthCard('Stop Hook', hooksReady, hooksReady ? '停止事件已接入' : '需要安装或修复')}
      ${healthCard('Telegram', config.watch_enabled && telegram.daemon_running, watcherText(config, telegram, recoverableThreads))}
    </section>

    <section class="work-grid primary-grid">
      <article class="panel recovery ${recoveryTone}">
        <div class="panel-head">
          <span class="panel-title">可见续跑</span>
          <span class="pill ${recoveryTone}">${escapeHtml(recoveryKindLabel(displayedDecision.kind))}</span>
        </div>
        <h2>${escapeHtml(displayedDecision.label)}</h2>
        <p>${escapeHtml(displayedDecision.reason)}</p>
        ${target ? `<p class="target-line">目标线程：${escapeHtml(target.thread.title || target.thread.id)} · ${escapeHtml(target.thread.id)}</p>` : ''}
        ${renderRecoverableThreads(recoverableThreads)}
        <div class="decision-strip">
          <span>路径：Codex APP 可见输入</span>
          <span>延迟：${displayedDecision.delay_seconds}s</span>
          <span>自动：${config.auto_recover ? '已开启' : '已关闭'}</span>
        </div>
        <button class="switch ${config.auto_recover ? 'on' : ''}" data-action="toggle-auto">
          ${config.auto_recover ? '关闭可见自动恢复' : '开启可见自动恢复'}
        </button>
      </article>

      <article class="panel active-thread">
        <div class="panel-head">
          <span class="panel-title">APP 内最后反馈</span>
          <span class="timestamp">${escapeHtml(formatCheckedAt(status.checked_at))}</span>
        </div>
        ${activeThread ? renderActiveThread(activeThread, payload.active_feedback, config.latest_feedback_enabled) : '<p class="empty">没有读取到最近的 Codex 线程。</p>'}
      </article>
    </section>

    <section class="work-grid secondary-grid">
      ${renderTelegramPanel(telegram, telegramDraft)}

      <article class="panel controls-panel">
        <div class="panel-head">
          <span class="panel-title">权限与 Hook</span>
          <button class="ghost" data-action="open-desktop-permissions">系统授权</button>
        </div>
        <div class="hook-steps desktop-steps">
          ${compactStep('accessibility', desktopControl.accessibility_granted ? 'ok' : 'error', '辅助功能')}
          ${compactStep('screen', desktopControl.screen_recording_granted ? 'ok' : 'optional', '屏幕录制')}
          ${compactStep('hook', hooksReady ? 'ok' : 'error', 'Stop Hook')}
          ${compactStep('installed-app', hooks.installed_app_command ? 'ok' : 'error', '安装路径')}
        </div>
        ${
          desktopControl.notes.length
            ? `<div class="notes">${desktopControl.notes.map((note) => `<p>${escapeHtml(note)}</p>`).join('')}</div>`
            : ''
        }
        ${renderHookSummary(hooks)}
        ${hookNotes.length ? `<div class="notes">${hookNotes.map((note) => `<p>${escapeHtml(note)}</p>`).join('')}</div>` : ''}
        ${renderHookDiagnostics(hooks)}
        <details class="advanced-settings compact-details">
          <summary>运行参数</summary>
          ${renderRuntimeSettings(runtimeDraft, config.config_path)}
        </details>
      </article>
    </section>

    <section class="work-grid secondary-grid">
      <article class="panel">
        <div class="panel-head">
          <span class="panel-title">最近线程</span>
          <span class="timestamp">${status.recent_threads.length} 条</span>
        </div>
        ${renderNewThreadComposer()}
        <div class="thread-list">
          ${status.recent_threads.map(renderThread).join('') || '<p class="empty">暂无线程记录。</p>'}
        </div>
      </article>

      <article class="panel logs-panel">
        <div class="panel-head">
          <span class="panel-title">可行动日志</span>
          <span class="timestamp">~/.codex/logs_2.sqlite</span>
        </div>
        ${renderLogs(status)}
      </article>
    </section>
  `);

  bindActions(payload);
}

function renderShell(content: string) {
  return `
    <div class="page-shell">
      ${content}
    </div>
  `;
}

function logoMark() {
  return `
    <svg viewBox="0 0 36 36" aria-hidden="true">
      <rect x="5" y="5" width="26" height="26" rx="7"></rect>
      <path d="M13 14.5l4.2 3.5-4.2 3.5"></path>
      <path d="M19.5 22h5.5"></path>
    </svg>
  `;
}

function renderNotice() {
  if (state.error) {
    return `<div class="banner error">${escapeHtml(state.error)}</div>`;
  }
  if (state.notice) {
    return `<div class="banner ok">${escapeHtml(state.notice)}</div>`;
  }
  return '';
}

function healthCard(title: string, ok: boolean, detail: string) {
  return `
    <article class="health-card ${ok ? 'good' : 'bad'}">
      <div class="health-dot"></div>
      <div>
        <h3>${escapeHtml(title)}</h3>
        <p>${escapeHtml(detail)}</p>
      </div>
    </article>
  `;
}

function renderActiveThread(thread: ThreadSummary, feedback: ThreadFeedback | null, feedbackEnabled: boolean) {
  return `
    <h2>${escapeHtml(thread.title || 'Untitled thread')}</h2>
    <p class="thread-id">${escapeHtml(thread.id)}</p>
    <dl class="config-list compact">
      ${kv('工作目录', thread.cwd)}
      ${kv('最近更新', formatUnix(thread.updated_at))}
      ${kv('Rollout', thread.rollout_path)}
    </dl>
    ${
      !feedbackEnabled
        ? `<div class="feedback-box muted-box">
            <div>
              <strong>最后反馈采集已关闭</strong>
              <span>不扫描 rollout</span>
            </div>
            <p>面板刷新时只显示线程元数据。</p>
          </div>`
        : feedback
          ? `<div class="feedback-box">
            <div>
              <strong>最后反馈</strong>
              <span>${escapeHtml(feedback.timestamp ? formatCheckedAt(feedback.timestamp) : '无时间戳')}</span>
            </div>
            <p>${escapeHtml(truncate(feedback.text, 900))}</p>
          </div>`
        : ''
    }
  `;
}

function renderThread(thread: ThreadSummary) {
  return `
    <div class="thread-row command-thread" data-thread-id="${escapeHtml(thread.id)}">
      <div class="thread-main">
        <div>
          <strong>${escapeHtml(thread.title || 'Untitled thread')}</strong>
          <span>${escapeHtml(thread.cwd)}</span>
          <small>${escapeHtml(thread.id)}</small>
        </div>
        <time>${escapeHtml(formatUnix(thread.updated_at))}</time>
        <button class="tiny-danger" data-action="archive-thread" data-thread-id="${escapeHtml(thread.id)}">删除</button>
      </div>
      <div class="thread-command">
        <textarea data-thread-prompt="${escapeHtml(thread.id)}" rows="2" placeholder="追加指令到这个线程"></textarea>
        <button class="secondary" data-action="submit-thread" data-thread-id="${escapeHtml(thread.id)}">追加指令</button>
      </div>
    </div>
  `;
}

function renderNewThreadComposer() {
  return `
    <div class="new-thread-box">
      <label>
        <span>新线程指令</span>
        <textarea id="new-thread-prompt" rows="3" placeholder="输入要发送给新 Codex 线程的第一条指令">${escapeHtml(state.newThreadPrompt)}</textarea>
      </label>
      <label>
        <span>工作目录</span>
        <input id="new-thread-path" type="text" value="${escapeHtml(state.newThreadPath)}" placeholder="/Users/gosu/Documents" />
      </label>
      <button class="primary" data-action="start-new-thread">开启新线程</button>
    </div>
  `;
}

function renderRuntimeSettings(draft: RuntimeDraft, configPath: string) {
  return `
    <div class="form-grid runtime-grid">
      <label class="toggle-row">
        <input id="runtime-watch-enabled" type="checkbox" ${draft.watch_enabled ? 'checked' : ''} />
        <span>启用本地 watcher</span>
      </label>
      <label class="toggle-row">
        <input id="runtime-auto-recover" type="checkbox" ${draft.auto_recover ? 'checked' : ''} />
        <span>启用可见自动恢复</span>
      </label>
      <label class="toggle-row">
        <input id="runtime-latest-feedback" type="checkbox" ${draft.latest_feedback_enabled ? 'checked' : ''} />
        <span>面板采集 APP 最后反馈</span>
      </label>
      <label class="toggle-row">
        <input id="runtime-completion-notify" type="checkbox" ${draft.completion_notifications_enabled ? 'checked' : ''} />
        <span>正常完成推送 Telegram</span>
      </label>
      <label class="toggle-row">
        <input id="runtime-record-completion" type="checkbox" ${draft.record_normal_completion_events ? 'checked' : ''} />
        <span>记录正常完成 Hook 事件</span>
      </label>
      <label>
        <span>轮询间隔秒</span>
        <input id="runtime-poll" type="number" min="5" step="1" value="${draft.poll_interval_seconds}" />
      </label>
      <label>
        <span>恢复上限</span>
        <input id="runtime-max" type="number" min="1" step="1" value="${draft.max_recoveries_per_thread}" />
      </label>
      <label>
        <span>冷却秒</span>
        <input id="runtime-cooldown" type="number" min="0" step="1" value="${draft.cooldown_seconds}" />
      </label>
      <label>
        <span>Hook 事件行数</span>
        <input id="runtime-hook-lines" type="number" min="50" step="50" value="${draft.hook_event_max_lines}" />
      </label>
      <label>
        <span>Hook 冷却行数</span>
        <input id="runtime-cooldown-lines" type="number" min="50" step="50" value="${draft.hook_cooldown_max_lines}" />
      </label>
      <label>
        <span>控制队列行数</span>
        <input id="runtime-control-lines" type="number" min="50" step="50" value="${draft.control_queue_max_lines}" />
      </label>
      <label>
        <span>后台日志上限 MB</span>
        <input id="runtime-log-mb" type="number" min="1" step="1" value="${bytesToMb(draft.log_max_bytes)}" />
      </label>
      <label>
        <span>默认续跑指令</span>
        <textarea id="runtime-continue-prompt" rows="4">${escapeHtml(draft.continue_prompt)}</textarea>
      </label>
      <label>
        <span>工具失败指令</span>
        <textarea id="runtime-tool-prompt" rows="3">${escapeHtml(draft.tool_failure_prompt)}</textarea>
      </label>
      <button class="primary" data-action="save-runtime">保存运行参数</button>
      <p class="command-line">${escapeHtml(configPath)}</p>
    </div>
  `;
}

function renderRecoverableThreads(items: ThreadRecovery[]) {
  if (!items.length) {
    return '<p class="quiet-line">最近线程没有待自动恢复的错误。</p>';
  }
  return `
    <div class="recoverable-list">
      ${items
        .map(
          (item) => `
            <div class="recoverable-row">
              <div>
                <strong>${escapeHtml(item.thread.title || item.thread.id)}</strong>
                <span>${escapeHtml(item.thread.id)}</span>
              </div>
              <div>
                <b>${escapeHtml(item.decision.label)}</b>
                <time>${escapeHtml(formatUnix(item.event.ts))}</time>
              </div>
            </div>
          `,
        )
        .join('')}
    </div>
  `;
}

function renderLogs(status: SentinelStatus) {
  const logs = [
    ['Turn error', status.latest_turn_error],
    ['Model error', status.latest_model_error],
    ['Tool error', status.latest_tool_error],
    ['Stream retry', status.latest_stream_retry],
  ] as const;

  return `
    <div class="log-stack">
      ${logs
        .map(([label, event]) =>
          event
            ? `<details class="log-item" open>
                <summary>${escapeHtml(label)} · ${escapeHtml(event.target)}</summary>
                <p>${escapeHtml(event.thread_id ?? '<no thread>')}</p>
                <pre>${escapeHtml(truncate(event.body, 760))}</pre>
              </details>`
            : `<div class="log-empty">${escapeHtml(label)}：暂无</div>`,
        )
        .join('')}
    </div>
  `;
}

function renderHookSummary(hooks: HookStatus) {
  const latest = hooks.recent_events[0];
  const detail = latest
    ? `${formatCheckedAt(latest.ts)} · ${latest.action} · ${latest.decision_label}`
    : '暂无事件';
  return `
    <div class="hook-summary">
      <div>
        <strong>最近 Stop 事件</strong>
        <span>${escapeHtml(detail)}</span>
      </div>
      <div>
        <strong>诊断记录</strong>
        <span>${hooks.recent_events.length} 条</span>
      </div>
    </div>
  `;
}

function renderHookDiagnostics(hooks: HookStatus) {
  return `
    <details class="advanced-settings compact-details hook-diagnostics">
      <summary>Hook 诊断</summary>
      <div class="hook-steps">
        ${step('feature', hooks.feature_enabled ? 'ok' : 'error', 'features.hooks', hooks.config_path)}
        ${step('stop', hooks.stop_installed ? 'ok' : 'error', 'Stop Hook', hooks.hooks_path)}
        ${step('installed-app', hooks.installed_app_command ? 'ok' : 'error', '安装路径', hooks.current_executable)}
      </div>
      ${renderHookEvents(hooks.recent_events)}
    </details>
  `;
}

function renderHookEvents(events: HookEvent[]) {
  if (!events.length) {
    return '<p class="quiet-line">暂无 Stop Hook 事件。</p>';
  }
  return `
    <div class="hook-event-list">
      ${events
        .map(
          (event) => `
            <div class="hook-event ${hookActionTone(event.action)}">
              <div>
                <strong>${escapeHtml(event.action)} · ${escapeHtml(event.decision_label)}</strong>
                <span>${escapeHtml(formatCheckedAt(event.ts))} · ${escapeHtml(event.source)} · ${event.delay_seconds}s</span>
              </div>
              <small>${escapeHtml(event.session_id ?? '-')} / ${escapeHtml(event.turn_id ?? '-')}</small>
              <p>${escapeHtml(event.body || event.event_key)}</p>
            </div>
          `,
        )
        .join('')}
    </div>
  `;
}

function renderTelegramPanel(settings: TelegramSettings, draft: TelegramDraft) {
  const tokenHint = settings.token_configured
    ? `当前 token：${settings.bot_token_masked}，留空则保留`
    : '填入 BotFather 给你的 123456:ABC... token';
  const effectivePairCode = draft.pairing_code || state.pairCode;
  const pairCommand = `/pair ${effectivePairCode}`;
  return `
    <article class="panel telegram-panel">
      <div class="panel-head">
        <span class="panel-title">Telegram 机器人</span>
        <span class="pill ${settings.enabled && settings.token_configured ? 'good' : 'warn'}">
          ${settings.enabled && settings.token_configured ? '已配置' : '待配置'}
        </span>
      </div>
      <div class="form-grid">
        <label>
          <span>Bot Token</span>
          <input id="tg-token" type="password" autocomplete="off" placeholder="${escapeHtml(tokenHint)}" value="${escapeHtml(draft.bot_token)}" />
        </label>
      </div>
      <div class="pair-box">
        <div>
          <span class="mini-label">远程配对</span>
          <h3>${escapeHtml(pairCommand)}</h3>
          <p>点“保存配置/启动后台”后，在机器人里发送这条命令即可远程写入白名单；也可以点“开始配对”由桌面端等待 60 秒。</p>
        </div>
        <div class="pair-actions">
          <button class="primary" data-action="pair-telegram">${state.pairing ? '等待消息...' : '开始配对'}</button>
          <button class="secondary" data-action="copy-pair-command">复制命令</button>
          <button class="ghost" data-action="rotate-pair-code">换配对码</button>
        </div>
      </div>
      <details class="advanced-settings">
        <summary>高级：手动白名单</summary>
        <div class="form-grid">
          <label class="toggle-row">
            <input id="tg-enabled" type="checkbox" ${draft.enabled ? 'checked' : ''} />
            <span>启用 Telegram 控制面</span>
          </label>
          <label class="toggle-row">
            <input id="tg-pairing-enabled" type="checkbox" ${draft.pairing_enabled ? 'checked' : ''} />
            <span>允许远程 /pair 配对</span>
          </label>
          <label>
            <span>远程配对码</span>
            <textarea id="tg-pairing-code" rows="1" placeholder="保存后 daemon 可识别 /pair 123456">${escapeHtml(effectivePairCode)}</textarea>
          </label>
          <label>
            <span>允许操作的 user_id</span>
            <textarea id="tg-users" rows="2" placeholder="配对后自动填写。多个 ID 用逗号或空格分隔。">${escapeHtml(draft.allowed_user_ids)}</textarea>
          </label>
          <label>
            <span>主动推送的 chat_id</span>
            <textarea id="tg-chats" rows="2" placeholder="配对后自动填写。群组通常是 -100 开头。">${escapeHtml(draft.allowed_chat_ids)}</textarea>
          </label>
        </div>
      </details>
      <div class="telegram-actions">
        <button class="primary" data-action="save-telegram">保存配置</button>
        <button class="secondary" data-action="test-telegram">测试 Token</button>
        <button class="secondary" data-action="send-telegram-test">发测试消息</button>
        <button class="ghost" data-action="start-telegram-daemon">${settings.daemon_running ? '后台运行中' : '启动后台'}</button>
      </div>
      <p class="command-line">常用命令：/pair ${escapeHtml(effectivePairCode)} /menu /status /threads /new /continue /delete /clear_archived。线程详情里可点“输入指令”或“删除线程”。</p>
      <dl class="config-list compact telegram-meta">
        ${kv('后台状态', settings.daemon_running ? '运行中' : '未运行')}
        ${kv('配置文件', settings.config_path)}
        ${kv('后台日志', settings.daemon_log_path)}
      </dl>
    </article>
  `;
}

type HookStepTone = 'ok' | 'optional' | 'error';

function actionableHookNotes(notes: string[]) {
  return notes
    .map((note) =>
      note
        .replace(
          'Codex hooks feature flag is not enabled in ~/.codex/config.toml.',
          'Codex hooks 功能未启用，请在 ~/.codex/config.toml 开启 features.hooks。',
        )
        .replace(
          'Stop hook is not installed; Codex cannot auto-continue from lifecycle stop events.',
          'Stop hook 未安装，Codex Sentinel 无法在任务停止事件里自动续跑。',
        )
        .trim(),
    )
    .filter(Boolean);
}

function hookActionTone(action: string) {
  if (action === 'continue') {
    return 'ok';
  }
  if (action === 'deduped' || action === 'deferred') {
    return 'warn';
  }
  if (action === 'manual' || action === 'manual_reauth' || action === 'loop_prevented') {
    return 'bad';
  }
  return '';
}

function compactStep(key: string, tone: HookStepTone, title: string) {
  const mark = tone === 'ok' ? '✓' : tone === 'optional' ? 'i' : '!';
  return `
    <div class="hook-compact ${tone}" data-step="${key}">
      <span>${mark}</span>
      <strong>${escapeHtml(title)}</strong>
    </div>
  `;
}

function step(key: string, tone: HookStepTone, title: string, path: string) {
  const mark = tone === 'ok' ? '✓' : tone === 'optional' ? 'i' : '!';
  return `
    <div class="hook-step ${tone}" data-step="${key}">
      <span>${mark}</span>
      <div>
        <strong>${escapeHtml(title)}</strong>
        <small>${escapeHtml(path)}</small>
      </div>
    </div>
  `;
}

function kv(key: string, value: string) {
  return `
    <div>
      <dt>${escapeHtml(key)}</dt>
      <dd>${escapeHtml(value || '-')}</dd>
    </div>
  `;
}

function bindActions(payload: DashboardPayload) {
  bindTelegramInputs();
  bindRuntimeInputs();
  bindNewThreadInputs();
  app.querySelector('[data-action="refresh"]')?.addEventListener('click', () => void loadDashboard());
  app.querySelector('[data-action="install-hooks"]')?.addEventListener('click', () => void installHooks());
  app
    .querySelector('[data-action="clear-archived"]')
    ?.addEventListener('click', () => void clearArchivedThreadsFromPanel());
  app.querySelector('[data-action="continue"]')?.addEventListener('click', () => void continueThread());
  app.querySelector('[data-action="start-new-thread"]')?.addEventListener('click', () => void startNewThreadFromPanel());
  app.querySelector('[data-action="save-runtime"]')?.addEventListener('click', () => void saveRuntimeSettings());
  app.querySelector('[data-action="save-telegram"]')?.addEventListener('click', () => void saveTelegramSettings());
  app.querySelector('[data-action="test-telegram"]')?.addEventListener('click', () => void testTelegramBot());
  app.querySelector('[data-action="pair-telegram"]')?.addEventListener('click', () => void pairTelegramBot());
  app.querySelector('[data-action="copy-pair-command"]')?.addEventListener('click', () => void copyPairCommand());
  app.querySelector('[data-action="rotate-pair-code"]')?.addEventListener('click', rotatePairCode);
  app
    .querySelector('[data-action="send-telegram-test"]')
    ?.addEventListener('click', () => void sendTelegramTestMessage());
  app
    .querySelector('[data-action="start-telegram-daemon"]')
    ?.addEventListener('click', () => void startTelegramDaemon());
  app
    .querySelector('[data-action="open-desktop-permissions"]')
    ?.addEventListener('click', () => void openDesktopPermissions());
  app.querySelector('[data-action="toggle-auto"]')?.addEventListener('click', () =>
    void toggleAutoRecover(!payload.config.auto_recover),
  );
  app.querySelectorAll('[data-action="submit-thread"]').forEach((element) => {
    element.addEventListener('click', () => {
      const thread = threadById(payload, element.getAttribute('data-thread-id'));
      if (thread) {
        void submitThreadInstruction(thread);
      }
    });
  });
  app.querySelectorAll('[data-action="archive-thread"]').forEach((element) => {
    element.addEventListener('click', () => {
      const thread = threadById(payload, element.getAttribute('data-thread-id'));
      if (thread) {
        void archiveThreadFromPanel(thread);
      }
    });
  });
  app.querySelector('[data-action="reveal-config"]')?.addEventListener('click', () => void revealConfigDir());
}

function bindTelegramInputs() {
  const update = () => {
    state.telegramDraft = collectTelegramInput();
  };
  app.querySelector('#tg-enabled')?.addEventListener('change', update);
  app.querySelector('#tg-pairing-enabled')?.addEventListener('change', update);
  app.querySelector('#tg-pairing-code')?.addEventListener('input', update);
  app.querySelector('#tg-token')?.addEventListener('input', update);
  app.querySelector('#tg-users')?.addEventListener('input', update);
  app.querySelector('#tg-chats')?.addEventListener('input', update);
}

function bindRuntimeInputs() {
  const update = () => {
    state.runtimeDraft = collectRuntimeInput();
  };
  [
    '#runtime-watch-enabled',
    '#runtime-auto-recover',
    '#runtime-latest-feedback',
    '#runtime-completion-notify',
    '#runtime-record-completion',
    '#runtime-poll',
    '#runtime-max',
    '#runtime-cooldown',
    '#runtime-hook-lines',
    '#runtime-cooldown-lines',
    '#runtime-control-lines',
    '#runtime-log-mb',
    '#runtime-rollout-backup-mb',
    '#runtime-continue-prompt',
    '#runtime-tool-prompt',
  ].forEach((selector) => app.querySelector(selector)?.addEventListener('input', update));
  app.querySelector('#runtime-watch-enabled')?.addEventListener('change', update);
  app.querySelector('#runtime-auto-recover')?.addEventListener('change', update);
  app.querySelector('#runtime-latest-feedback')?.addEventListener('change', update);
  app.querySelector('#runtime-completion-notify')?.addEventListener('change', update);
  app.querySelector('#runtime-record-completion')?.addEventListener('change', update);
}

function bindNewThreadInputs() {
  app.querySelector('#new-thread-prompt')?.addEventListener('input', () => {
    state.newThreadPrompt = app.querySelector<HTMLTextAreaElement>('#new-thread-prompt')?.value ?? '';
  });
  app.querySelector('#new-thread-path')?.addEventListener('input', () => {
    state.newThreadPath = app.querySelector<HTMLInputElement>('#new-thread-path')?.value ?? '';
  });
}

function collectTelegramInput(): TelegramDraft {
  return {
    enabled: Boolean(app.querySelector<HTMLInputElement>('#tg-enabled')?.checked),
    bot_token: app.querySelector<HTMLInputElement>('#tg-token')?.value ?? state.telegramDraft?.bot_token ?? '',
    pairing_enabled:
      app.querySelector<HTMLInputElement>('#tg-pairing-enabled')?.checked ??
      state.telegramDraft?.pairing_enabled ??
      true,
    pairing_code:
      app.querySelector<HTMLTextAreaElement>('#tg-pairing-code')?.value ??
      state.telegramDraft?.pairing_code ??
      state.pairCode,
    allowed_user_ids:
      app.querySelector<HTMLTextAreaElement>('#tg-users')?.value ?? state.telegramDraft?.allowed_user_ids ?? '',
    allowed_chat_ids:
      app.querySelector<HTMLTextAreaElement>('#tg-chats')?.value ?? state.telegramDraft?.allowed_chat_ids ?? '',
  };
}

function collectRuntimeInput(): RuntimeDraft {
  const fallback = state.runtimeDraft ?? draftFromConfig(state.payload!.config);
  return {
    watch_enabled:
      app.querySelector<HTMLInputElement>('#runtime-watch-enabled')?.checked ?? fallback.watch_enabled,
    auto_recover:
      app.querySelector<HTMLInputElement>('#runtime-auto-recover')?.checked ?? fallback.auto_recover,
    latest_feedback_enabled:
      app.querySelector<HTMLInputElement>('#runtime-latest-feedback')?.checked ?? fallback.latest_feedback_enabled,
    completion_notifications_enabled:
      app.querySelector<HTMLInputElement>('#runtime-completion-notify')?.checked ??
      fallback.completion_notifications_enabled,
    record_normal_completion_events:
      app.querySelector<HTMLInputElement>('#runtime-record-completion')?.checked ??
      fallback.record_normal_completion_events,
    poll_interval_seconds: numericInput('#runtime-poll', fallback.poll_interval_seconds),
    max_recoveries_per_thread: numericInput('#runtime-max', fallback.max_recoveries_per_thread),
    cooldown_seconds: numericInput('#runtime-cooldown', fallback.cooldown_seconds),
    hook_event_max_lines: numericInput('#runtime-hook-lines', fallback.hook_event_max_lines),
    hook_cooldown_max_lines: numericInput('#runtime-cooldown-lines', fallback.hook_cooldown_max_lines),
    control_queue_max_lines: numericInput('#runtime-control-lines', fallback.control_queue_max_lines),
    log_max_bytes: mbToBytes(numericInput('#runtime-log-mb', bytesToMb(fallback.log_max_bytes))),
    continue_prompt:
      app.querySelector<HTMLTextAreaElement>('#runtime-continue-prompt')?.value ?? fallback.continue_prompt,
    tool_failure_prompt:
      app.querySelector<HTMLTextAreaElement>('#runtime-tool-prompt')?.value ?? fallback.tool_failure_prompt,
  };
}

function draftFromTelegram(settings: TelegramSettings): TelegramDraft {
  return {
    enabled: settings.enabled || settings.token_configured,
    bot_token: '',
    pairing_enabled: settings.pairing_enabled,
    pairing_code: settings.pairing_code || state.pairCode,
    allowed_user_ids: settings.allowed_user_ids,
    allowed_chat_ids: settings.allowed_chat_ids,
  };
}

function draftFromConfig(config: ConfigSummary): RuntimeDraft {
  return {
    watch_enabled: config.watch_enabled,
    poll_interval_seconds: config.poll_interval_seconds,
    auto_recover: config.auto_recover,
    max_recoveries_per_thread: config.max_recoveries_per_thread,
    cooldown_seconds: config.cooldown_seconds,
    continue_prompt: config.continue_prompt,
    tool_failure_prompt: config.tool_failure_prompt,
    latest_feedback_enabled: config.latest_feedback_enabled,
    completion_notifications_enabled: config.completion_notifications_enabled,
    record_normal_completion_events: config.record_normal_completion_events,
    hook_event_max_lines: config.hook_event_max_lines,
    hook_cooldown_max_lines: config.hook_cooldown_max_lines,
    control_queue_max_lines: config.control_queue_max_lines,
    log_max_bytes: config.log_max_bytes,
  };
}

function numericInput(selector: string, fallback: number) {
  const value = Number(app.querySelector<HTMLInputElement>(selector)?.value);
  return Number.isFinite(value) ? value : fallback;
}

function bytesToMb(value: number) {
  return Math.max(1, Math.round(value / 1024 / 1024));
}

function mbToBytes(value: number) {
  return Math.max(0, Math.round(value * 1024 * 1024));
}

function threadById(payload: DashboardPayload, threadId: string | null) {
  if (!threadId) {
    return null;
  }
  return payload.status.recent_threads.find((thread) => thread.id === threadId) ?? null;
}

function createPairCode() {
  return String(Math.floor(100000 + Math.random() * 900000));
}

function hasEditableFocus() {
  const active = document.activeElement;
  if (!active) {
    return false;
  }
  const tag = active.tagName.toLowerCase();
  return tag === 'input' || tag === 'textarea' || active.getAttribute('contenteditable') === 'true';
}

function watcherText(config: ConfigSummary, telegram: TelegramSettings, recoverableThreads: ThreadRecovery[]) {
  if (!config.watch_enabled) {
    return '本地 watcher 未启用';
  }
  if (!telegram.daemon_running) {
    return '本地 watcher 未运行';
  }
  if (recoverableThreads.length) {
    return `${recoverableThreads.length} 个线程待恢复`;
  }
  return '本地 watcher 运行中';
}

function recoveryKindLabel(kind: RecoveryKind) {
  const labels: Record<RecoveryKind, string> = {
    None: '正常',
    RetryLater: '延迟重试',
    RetrySoon: '短暂重试',
    ManualOnly: '人工处理',
    Reauth: '重新授权',
    SwitchModel: '切换模型',
    ToolRetryWithDifferentPath: '换路重试',
    SafetyBlocked: '安全拦截恢复',
  };
  return labels[kind] ?? kind;
}

function toneForRecovery(kind: RecoveryKind) {
  if (kind === 'None') {
    return 'good';
  }
  if (
    kind === 'RetryLater' ||
    kind === 'RetrySoon' ||
    kind === 'ToolRetryWithDifferentPath' ||
    kind === 'SafetyBlocked'
  ) {
    return 'warn';
  }
  return 'bad';
}

function formatCheckedAt(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString('zh-CN', { hour12: false });
}

function formatUnix(value: number) {
  const date = new Date(value * 1000);
  if (Number.isNaN(date.getTime())) {
    return String(value);
  }
  return date.toLocaleString('zh-CN', { hour12: false });
}

function truncate(value: string, max: number) {
  return value.length > max ? `${value.slice(0, max)}...` : value;
}

function stringifyError(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function escapeHtml(value: string) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#039;');
}

function cssEscape(value: string) {
  return typeof CSS !== 'undefined' && typeof CSS.escape === 'function'
    ? CSS.escape(value)
    : value.replaceAll('"', '\\"');
}

function mockDashboard(
  overrides: {
    hooksReady?: boolean;
    autoRecover?: boolean;
    telegramEnabled?: boolean;
    telegramDaemon?: boolean;
    telegramPaired?: boolean;
  } = {},
): DashboardPayload {
  const current = state.payload;
  const currentHooksReady = current
    ? current.hooks.feature_enabled && current.hooks.stop_installed
    : false;
  const hooksReady = overrides.hooksReady ?? currentHooksReady;
  const autoRecover = overrides.autoRecover ?? current?.config.auto_recover ?? false;
  const telegramEnabled = overrides.telegramEnabled ?? current?.telegram.enabled ?? true;
  const telegramDaemon = overrides.telegramDaemon ?? current?.telegram.daemon_running ?? false;
  const telegramPaired =
    overrides.telegramPaired ??
    (current ? Boolean(current.telegram.allowed_user_ids || current.telegram.allowed_chat_ids) : true);
  return {
    status: {
      checked_at: new Date().toISOString(),
      codex_running: true,
      recent_threads: [
        {
          id: '019e0845-856d-7c73-ad4a-2e46655aa611',
          title: '设计 Codex 监控 App',
          cwd: '/Users/gosu/Documents',
          updated_at: Math.floor(Date.now() / 1000) - 108,
          rollout_path: '/Users/gosu/.codex/sessions/2026/05/08/rollout-demo.jsonl',
        },
        {
          id: '019e0952-0982-70e1-b9e0-e98a6420fb4b',
          title: '了解 Codex hooks',
          cwd: '/Users/gosu/Documents',
          updated_at: Math.floor(Date.now() / 1000) - 860,
          rollout_path: '/Users/gosu/.codex/sessions/2026/05/09/rollout-hooks.jsonl',
        },
      ],
      latest_turn_error: {
        ts: Math.floor(Date.now() / 1000) - 920,
        level: 'INFO',
        target: 'codex_core::session::turn',
        thread_id: '019e0845-856d-7c73-ad4a-2e46655aa611',
        body: 'Turn error: exceeded retry limit, last status: 429 Too Many Requests',
      },
      latest_model_error: null,
      latest_stream_retry: {
        ts: Math.floor(Date.now() / 1000) - 550,
        level: 'WARN',
        target: 'codex_core::session::turn',
        thread_id: '019e0845-856d-7c73-ad4a-2e46655aa611',
        body: 'stream disconnected - retrying sampling request (1/5 in 207ms)...',
      },
      latest_tool_error: null,
      recovery: {
        kind: 'RetryLater',
        auto_allowed: true,
        delay_seconds: 5,
        label: 'Rate limited',
        reason: 'Codex already exhausted its internal retry loop. Retry with a short backoff.',
      },
    },
    config: {
      config_path: '/Users/gosu/.codex-sentinel/config.toml',
      config_dir: '/Users/gosu/.codex-sentinel',
      telegram_enabled: telegramEnabled,
      telegram_token_configured: true,
      allowed_user_count: telegramPaired ? 1 : 0,
      allowed_chat_count: telegramPaired ? 1 : 0,
      watch_enabled: true,
      poll_interval_seconds: 5,
      auto_recover: autoRecover,
      max_recoveries_per_thread: 10,
      cooldown_seconds: 5,
      continue_prompt: '继续干。请先检查当前线程最近状态和工具输出，不要从头开始。',
      tool_failure_prompt: '继续干。上一条工具调用失败了，请换一种方式继续完成任务。',
      latest_feedback_enabled: true,
      completion_notifications_enabled: true,
      record_normal_completion_events: false,
      hook_event_max_lines: 500,
      hook_cooldown_max_lines: 1000,
      control_queue_max_lines: 1000,
      log_max_bytes: 5 * 1024 * 1024,
    },
    hooks: {
      feature_enabled: hooksReady,
      config_path: '/Users/gosu/.codex/config.toml',
      hooks_path: '/Users/gosu/.codex/hooks.json',
      hooks_file_exists: hooksReady,
      stop_installed: hooksReady,
      installed_app_command: hooksReady,
      current_executable: '/Applications/Codex Sentinel.app/Contents/MacOS/codex-sentinel',
      installed_commands: hooksReady
        ? ['"/Applications/Codex Sentinel.app/Contents/MacOS/codex-sentinel" hook-stop']
        : [],
      recent_events: hooksReady
        ? [
            {
              ts: new Date(Date.now() - 90_000).toISOString(),
              event: 'Stop',
              action: 'continue',
              event_key: 'demo:turn:RetrySoon:1234',
              source: 'last_assistant_message',
              session_id: '019e0845-856d-7c73-ad4a-2e46655aa611',
              turn_id: 'turn-demo',
              delay_seconds: 3,
              decision_label: 'Temporary upstream failure',
              decision_kind: 'RetrySoon',
              body: 'Turn error: unexpected status 503 Service Unavailable',
            },
            {
              ts: new Date(Date.now() - 40_000).toISOString(),
              event: 'Stop',
              action: 'deduped',
              event_key: 'demo:turn:RetrySoon:1234',
              source: 'last_assistant_message',
              session_id: '019e0845-856d-7c73-ad4a-2e46655aa611',
              turn_id: 'turn-demo',
              delay_seconds: 3,
              decision_label: 'Temporary upstream failure',
              decision_kind: 'RetrySoon',
              body: '同一个 Stop 事件在冷却窗口内被抑制。',
            },
          ]
        : [],
      notes: hooksReady
        ? []
        : [
            'Codex hooks feature flag is not enabled in ~/.codex/config.toml.',
            'Stop hook is not installed; Codex cannot auto-continue from lifecycle stop events.',
          ],
    },
    telegram: {
      enabled: telegramEnabled,
      bot_token_masked: '123456:••••demo',
      token_configured: true,
      allowed_user_ids: telegramPaired ? '123456789' : '',
      allowed_chat_ids: telegramPaired ? '123456789' : '',
      pairing_enabled: true,
      pairing_code: state.pairCode,
      daemon_running: telegramDaemon,
      config_path: '/Users/gosu/.codex-sentinel/config.toml',
      daemon_log_path: '/Users/gosu/.codex-sentinel/telegram-daemon.out.log',
    },
    desktop_control: {
      mode: 'visible_desktop',
      accessibility_granted: hooksReady,
      screen_recording_granted: true,
      notes: hooksReady
        ? []
        : ['需要在系统设置 -> 隐私与安全性 -> 辅助功能 中允许 Codex Sentinel，才能在 Codex APP 可见窗口内点击和输入。'],
    },
    recoverable_threads: [],
    active_feedback: {
      thread_id: '019e0845-856d-7c73-ad4a-2e46655aa611',
      title: '设计 Codex 监控 App',
      timestamp: new Date().toISOString(),
      text: '已完成最近一次恢复。后续续跑会打开 Codex APP 并在可见输入框发送；如果需要人工决策，会在这里显示最后反馈。',
    },
  };
}
