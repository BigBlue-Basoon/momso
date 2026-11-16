import { useCallback, useEffect, useMemo, useState } from 'react'
import { W } from '../words'
import { Check, Gauge, KeyRound, LoaderCircle, LogIn, RefreshCw, Wrench } from 'lucide-react'
import { ProviderMark } from './ProviderMark'
import type { AppPreferences, AssistantAuthMode, AssistantProvider } from '../preferences'
import {
  connectExternalProvider,
  getAiConnectionOverview,
  getAiUsageOverview,
  installExternalProvider,
  removeAiApiKey,
  storeAiApiKey,
  type ConnectionOverview,
  type AiUsageOverview,
} from '../runtime'

// 예전 사용량 칸을 다시 보이려면 true.
const SHOW_OLD_USAGE_SECTION = false as boolean

type AiConnectionSettingsProps = {
  preferences: AppPreferences
  onPreferenceChange: <K extends keyof AppPreferences>(key: K, value: AppPreferences[K]) => void
}

const PROVIDERS: Array<{ id: AssistantProvider; label: string; detail: string }> = [
  { id: 'codex', label: W.provider.codex, detail: W.aiConnectionSettings.text1 },
  { id: 'claude', label: W.provider.claude, detail: W.aiConnectionSettings.text2 },
]

const AUTH_MODES: Array<{ id: AssistantAuthMode; label: string; detail: string }> = [
  { id: 'subscription', label: W.aiConnectionSettings.text3, detail: W.aiConnectionSettings.text4 },
  { id: 'api_key', label: W.aiConnectionSettings.text5, detail: W.aiConnectionSettings.text6 },
]

export function AiConnectionSettings({ preferences, onPreferenceChange }: AiConnectionSettingsProps) {
  const [overview, setOverview] = useState<ConnectionOverview | null>(null)
  const [usage, setUsage] = useState<AiUsageOverview | null>(null)
  const [usageLoading, setUsageLoading] = useState(false)
  const [apiKey, setApiKey] = useState('')
  const [working, setWorking] = useState(false)
  const [message, setMessage] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setUsageLoading(true)
    setUsage(null)
    try {
      const [nextOverview, nextUsage] = await Promise.all([
        getAiConnectionOverview(),
        getAiUsageOverview(preferences.assistantProvider, preferences.assistantAuthMode),
      ])
      setOverview(nextOverview)
      setUsage(nextUsage)
      setMessage(null)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setUsageLoading(false)
    }
  }, [preferences.assistantAuthMode, preferences.assistantProvider])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const tool = useMemo(
    () => overview?.tools.find((candidate) => candidate.id === preferences.assistantProvider) ?? null,
    [overview, preferences.assistantProvider],
  )
  const keyProvider = preferences.assistantProvider === 'codex' ? 'openai' : 'anthropic'
  const keyConfigured = keyProvider === 'openai'
    ? overview?.openaiApiKeyConfigured
    : overview?.anthropicApiKeyConfigured
  const loadingOverview = overview === null

  async function connectSelectedProvider() {
    setWorking(true)
    setMessage(null)
    try {
      if (!tool?.installed) await installExternalProvider(preferences.assistantProvider)
      await connectExternalProvider(preferences.assistantProvider)
      await refresh()
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setWorking(false)
    }
  }

  async function saveKey() {
    setWorking(true)
    setMessage(null)
    try {
      await storeAiApiKey(keyProvider, apiKey)
      setApiKey('')
      await refresh()
      setMessage(W.aiConnectionSettings.text7)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setWorking(false)
    }
  }

  async function removeKey() {
    setWorking(true)
    setMessage(null)
    try {
      await removeAiApiKey(keyProvider)
      setApiKey('')
      await refresh()
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setWorking(false)
    }
  }

  return (
    <div className="ai-connection-settings">
      <div className="settings-choice-grid connection-choice-grid" role="radiogroup" aria-label={W.aiConnectionSettings.aria1}>
        {PROVIDERS.map((provider) => (
          <label
            key={provider.id}
            className={preferences.assistantProvider === provider.id ? 'settings-choice active' : 'settings-choice'}
          >
            <input
              type="radio"
              name="assistant-provider"
              checked={preferences.assistantProvider === provider.id}
              onChange={() => onPreferenceChange('assistantProvider', provider.id)}
            />
            <span>
              <strong>{provider.label}</strong>
              <small>{provider.detail}</small>
            </span>
            {preferences.assistantProvider === provider.id && <Check size={15} aria-hidden />}
          </label>
        ))}
      </div>

      <div className="settings-choice-grid connection-choice-grid" role="radiogroup" aria-label={W.aiConnectionSettings.aria2}>
        {AUTH_MODES.map((mode) => (
          <label
            key={mode.id}
            className={preferences.assistantAuthMode === mode.id ? 'settings-choice active' : 'settings-choice'}
          >
            <input
              type="radio"
              name="assistant-auth-mode"
              checked={preferences.assistantAuthMode === mode.id}
              onChange={() => onPreferenceChange('assistantAuthMode', mode.id)}
            />
            <span>
              <strong>{mode.label}</strong>
              <small>{mode.detail}</small>
            </span>
            {preferences.assistantAuthMode === mode.id && <Check size={15} aria-hidden />}
          </label>
        ))}
      </div>

      {preferences.assistantAuthMode === 'subscription' ? (
        <div className="connection-action-row">
          {/* 예전 로봇 그림(회사 대신 그린 것)을 빼고 그 회사의 공식 CI 를 감싸지 않고 둔다(SCREEN_RULES 2·4절). */}
          <ProviderMark provider={preferences.assistantProvider === 'claude' ? 'claude' : 'chatgpt'} className="connection-mark" />
          <div>
            <strong>{(tool ? W.provider[tool.id as 'codex' | 'claude'] ?? tool.label : null) ?? (preferences.assistantProvider === 'codex' ? W.provider.codex : W.provider.claude)}</strong>
            <p>{loadingOverview ? W.aiConnectionSettings.text8 : tool?.authenticated ? W.aiConnectionSettings.text9 : tool?.installed ? W.aiConnectionSettings.text10 : W.aiConnectionSettings.text11}</p>
          </div>
          <span className={`connection-state ${tool?.authenticated ? 'ready' : ''}`}>
            {loadingOverview ? W.aiConnectionSettings.text12 : tool?.authenticated ? W.aiConnectionSettings.text13 : tool?.installed ? W.aiConnectionSettings.text14 : W.aiConnectionSettings.text15}
          </span>
          {!loadingOverview && !tool?.authenticated && (
            <button className="secondary-button" type="button" disabled={working} onClick={() => void connectSelectedProvider()}>
              {working ? <LoaderCircle size={15} className="is-spinning" aria-hidden /> : tool?.installed ? <LogIn size={15} aria-hidden /> : <Wrench size={15} aria-hidden />}
              {working ? W.aiConnectionSettings.text16 : tool?.installed ? W.aiConnectionSettings.text17 : W.aiConnectionSettings.text18}
            </button>
          )}
          {tool?.authenticated && (
            <button className="icon-button subtle" type="button" onClick={() => void refresh()} aria-label={W.aiConnectionSettings.aria3} title={W.aiConnectionSettings.aria3}>
              <RefreshCw size={15} aria-hidden />
            </button>
          )}
          {loadingOverview && <LoaderCircle size={15} className="is-spinning connection-checking" aria-label={W.aiConnectionSettings.aria4} />}
        </div>
      ) : (
        <div className="api-key-editor">
          <span className="connection-icon" aria-hidden><KeyRound size={17} /></span>
          <label>
            <span>{keyProvider === 'openai' ? W.aiConnectionSettings.text19 : W.aiConnectionSettings.text20}</span>
            <input
              type="password"
              value={apiKey}
              autoComplete="off"
              spellCheck={false}
              placeholder={keyConfigured ? W.aiConnectionSettings.hint1 : W.aiConnectionSettings.hint2}
              onChange={(event) => setApiKey(event.target.value)}
            />
          </label>
          <span className={`connection-state ${keyConfigured ? 'ready' : ''}`}>
            {loadingOverview ? W.aiConnectionSettings.text12 : keyConfigured ? W.aiConnectionSettings.text21 : W.aiConnectionSettings.text22}
          </span>
          <div className="inline-actions">
            {keyConfigured && (
              <button className="text-button danger" type="button" disabled={working} onClick={() => void removeKey()}>{W.aiConnectionSettings.text23}</button>
            )}
            <button className="secondary-button" type="button" disabled={working || !apiKey.trim()} onClick={() => void saveKey()}>
              <KeyRound size={14} aria-hidden />{W.aiConnectionSettings.text24}</button>
          </div>
        </div>
      )}

      {/* 예전 사용량 칸은 화면에서 치웠다(2026-11-15, 본부). 사용량은 아이콘 띠 한 곳에만 둔다. 코드는 남긴다. */}
      {SHOW_OLD_USAGE_SECTION && <AiUsageSection
        authMode={preferences.assistantAuthMode}
        loading={usageLoading}
        overview={usage}
        onRefresh={() => void refresh()}
      />}

      {message && <div className="inline-status" role="status">{message}</div>}
    </div>
  )
}

type AiUsageSectionProps = {
  authMode: AssistantAuthMode
  loading: boolean
  overview: AiUsageOverview | null
  onRefresh: () => void
}

function AiUsageSection({ authMode, loading, overview, onRefresh }: AiUsageSectionProps) {
  return (
    <section className="ai-usage" aria-labelledby="ai-usage-title">
      <div className="ai-usage-heading">
        <span className="connection-icon" aria-hidden><Gauge size={17} /></span>
        <div>
          <strong id="ai-usage-title">{W.aiConnectionSettings.text25}</strong>
          <p>{authMode === 'subscription' ? W.aiConnectionSettings.text26 : W.aiConnectionSettings.text27}</p>
        </div>
        <button
          className="icon-button subtle"
          type="button"
          disabled={loading}
          onClick={onRefresh}
          aria-label={W.aiConnectionSettings.aria5}
          title={W.aiConnectionSettings.aria5}
        >
          {loading ? <LoaderCircle size={15} className="is-spinning" aria-hidden /> : <RefreshCw size={15} aria-hidden />}
        </button>
      </div>

      {authMode === 'subscription' ? (
        <SubscriptionUsage overview={overview} loading={loading} />
      ) : (
        <ApiUsage overview={overview} loading={loading} />
      )}
    </section>
  )
}

function SubscriptionUsage({ overview, loading }: Pick<AiUsageSectionProps, 'overview' | 'loading'>) {
  if (!overview && loading) {
    return <div className="ai-usage-notice" role="status">{W.aiConnectionSettings.text28}</div>
  }
  if (!overview) {
    return <div className="ai-usage-notice">{W.aiConnectionSettings.text29}</div>
  }

  return (
    <div className="quota-list">
      {overview.quota.status === 'available' && overview.quota.windows.map((window) => {
        const remaining = Math.max(0, Math.min(100, 100 - window.usedPercent))
        return (
          <div className="quota-row" key={window.id}>
            <div className="quota-label">
              <strong>{window.label}</strong>
              <span>{W.aiConnectionSettings.text30(formatPercent(remaining))}</span>
            </div>
            <div
              className="quota-track"
              role="progressbar"
              aria-label={W.aiConnectionSettings.aria6(window.label)}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(remaining)}
            >
              <span className="quota-fill" style={{ width: `${remaining}%` }} />
            </div>
            <small>{window.resetsAt ? W.aiConnectionSettings.text31(formatResetTime(window.resetsAt)) : W.aiConnectionSettings.text32}</small>
          </div>
        )
      })}
      {overview.quota.status !== 'available' && (
        <div className="ai-usage-notice">{overview.quota.detail ?? W.aiConnectionSettings.text33}</div>
      )}
      <p className="usage-local-total">{W.aiConnectionSettings.text34(overview.monthRequestCount.toLocaleString('ko-KR'), formatTokens(overview.monthTotalTokens))}</p>
    </div>
  )
}

function ApiUsage({ overview, loading }: Pick<AiUsageSectionProps, 'overview' | 'loading'>) {
  if (!overview && loading) {
    return <div className="ai-usage-notice" role="status">{W.aiConnectionSettings.text35}</div>
  }
  const lastCost = overview?.lastRequest?.estimatedCostUsdMicros ?? null
  return (
    <div>
      <div className="usage-cost-grid">
        <UsageMetric label={W.aiConnectionSettings.label1} value={formatUsd(lastCost)} />
        <UsageMetric label={W.aiConnectionSettings.label2} value={formatUsd(overview?.todayEstimatedCostUsdMicros ?? 0)} />
        <UsageMetric label={W.aiConnectionSettings.label3} value={formatUsd(overview?.monthEstimatedCostUsdMicros ?? 0)} />
      </div>
      <p className="usage-local-total">{W.aiConnectionSettings.text36(overview?.monthRequestCount.toLocaleString('ko-KR') ?? '0', formatTokens(overview?.monthTotalTokens ?? 0))}</p>
    </div>
  )
}

function UsageMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="usage-metric">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{W.aiConnectionSettings.text37}</small>
    </div>
  )
}

function formatUsd(micros: number | null) {
  if (micros === null) return W.aiConnectionSettings.text38
  const dollars = micros / 1_000_000
  return `$${dollars < 0.01 ? dollars.toFixed(4) : dollars.toFixed(2)}`
}

function formatTokens(tokens: number) {
  return W.aiConnectionSettings.text39(new Intl.NumberFormat('ko-KR', { notation: 'compact', maximumFractionDigits: 1 }).format(tokens))
}

function formatPercent(value: number) {
  return `${new Intl.NumberFormat('ko-KR', { maximumFractionDigits: 1 }).format(value)}%`
}

function formatResetTime(timestamp: number) {
  return new Intl.DateTimeFormat('ko-KR', {
    month: 'numeric',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(new Date(timestamp * 1000))
}
