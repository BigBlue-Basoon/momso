import { useState } from 'react'
import { W } from '../words'
import { ArrowRight, Check } from 'lucide-react'
import { ProviderMark } from './ProviderMark'
import './AiWelcome.css'

const providers = [
  { id: 'chatgpt', name: 'ChatGPT', company: 'OpenAI', strength: W.aiWelcome.text1, good: W.aiWelcome.text2, tradeoff: W.aiWelcome.text3 },
  { id: 'claude', name: W.provider.claude, company: 'Anthropic', strength: W.aiWelcome.text4, good: W.aiWelcome.text5, tradeoff: W.aiWelcome.text6 },
  { id: 'gemini', name: W.provider.gemini, company: 'Google', strength: W.aiWelcome.text7, good: W.aiWelcome.text8, tradeoff: W.aiWelcome.text9 },
] as const

export function AiWelcome() {
  const [selected, setSelected] = useState<(typeof providers)[number]['id'] | null>(null)
  const [claudeHovered, setClaudeHovered] = useState(false)
  const [claudeFocused, setClaudeFocused] = useState(false)
  const choice = providers.find((provider) => provider.id === selected)

  return (
    <main className="ai-welcome">
      <div className="ai-welcome-inner">
        <div className="ai-welcome-heading">
          <h1>{W.aiWelcome.text10}</h1>
          <p>{W.aiWelcome.text11}</p>
        </div>
        <div className="ai-provider-grid" role="radiogroup" aria-label={W.aiWelcome.aria1}>
          {providers.map((provider) => (
            <button key={provider.id} type="button" role="radio" aria-checked={selected === provider.id}
              className={`ai-provider ai-provider-${provider.id} provider-mark-trigger ${selected === provider.id ? 'is-selected' : ''}`}
              onPointerEnter={() => { if (provider.id === 'claude') setClaudeHovered(true) }}
              onPointerLeave={() => { if (provider.id === 'claude') setClaudeHovered(false) }}
              onFocus={() => { if (provider.id === 'claude') setClaudeFocused(true) }}
              onBlur={() => { if (provider.id === 'claude') setClaudeFocused(false) }}
              onClick={() => setSelected(provider.id)}>
              <span className="ai-provider-top"><ProviderMark provider={provider.id} active={provider.id === 'claude' && (claudeHovered || claudeFocused)} className="ai-provider-mark" /><span className="ai-provider-identity"><strong>{provider.name}</strong><small>{provider.company}</small></span><span className="ai-provider-check">{selected === provider.id && <Check size={16} />}</span></span>
              <span className="ai-provider-title">{provider.strength}</span>
              <span className="ai-provider-label">{W.aiWelcome.text12}</span><span className="ai-provider-copy">{provider.good}</span>
              <span className="ai-provider-label">{W.aiWelcome.text13}</span><span className="ai-provider-copy">{provider.tradeoff}</span>
            </button>
          ))}
        </div>
        <div className="ai-welcome-footer">
          <div><p>{W.aiWelcome.text14}</p><small>{W.aiWelcome.text15}</small></div>
          <button className="ai-welcome-continue" type="button" disabled>{choice ? W.aiWelcome.text16(choice.name) : W.aiWelcome.text17} <ArrowRight size={17} /></button>
        </div>
      </div>
    </main>
  )
}
