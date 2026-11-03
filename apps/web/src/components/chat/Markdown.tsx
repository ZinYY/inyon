import { memo, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import ReactMarkdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import clsx from 'clsx'
import { highlight, isLoaded, languageOf, loadLanguage } from './highlight.ts'

export { CopyButton, ReplyFont } from './copy.tsx'
import { CopyButton, ReplyFont } from './copy.tsx'

function textOf(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(textOf).join('')
  if (node && typeof node === 'object' && 'props' in node) return textOf((node as any).props.children)
  return ''
}

/** True once the block's language can be highlighted; fetches it the first time and re-renders then. */
function useLanguage(lang: string | undefined) {
  const [, arrived] = useState(0)
  const loaded = !!lang && isLoaded(lang)
  useEffect(() => {
    if (!lang || loaded) return
    let on = true
    loadLanguage(lang).then((ok) => { if (on && ok) arrived((n) => n + 1) })
    return () => { on = false }
  }, [lang, loaded])
  return loaded
}

const components: Components = {
  // the same markup rehype-highlight made (`code.hljs.language-x` with `hljs-*` spans), so the
  // stylesheet applies; the language's grammar arrives on demand instead of shipping with the chat
  code({ children, className, node: _node, ...rest }) {
    const lang = languageOf(className)
    const loaded = useLanguage(lang)
    const text = textOf(children)
    const html = useMemo(() => (loaded && lang ? highlight(lang, text) : ''), [loaded, lang, text])
    if (!loaded) return <code className={className} {...rest}>{children}</code>
    return <code className={clsx('hljs', className)} {...rest} dangerouslySetInnerHTML={{ __html: html }} />
  },
  pre({ children }) {
    const child: any = Array.isArray(children) ? children[0] : children
    const cls: string = child?.props?.className ?? ''
    const lang = cls.match(/language-([\w+#-]+)/)?.[1]
    const code = textOf(child?.props?.children).replace(/\n$/, '')
    return (
      <div className="group/code my-3 overflow-hidden rounded-xl border border-line bg-code">
        <div className="flex h-8 items-center justify-between border-b border-line pl-3.5 pr-1.5 font-sans text-[12px] text-fg-3">
          <span>{lang ?? 'text'}</span>
          <CopyButton text={code} />
        </div>
        <pre className="overflow-x-auto px-3.5 py-3 font-mono text-[12.5px] leading-[1.65]">{children}</pre>
      </div>
    )
  },
  table({ children }) {
    return (
      <div className="table-wrap">
        <table>{children}</table>
      </div>
    )
  },
  a({ children, href }) {
    return (
      <a href={href} target="_blank" rel="noreferrer noopener">
        {children}
      </a>
    )
  },
  // agent output can't load outside images: a prompt-injected agent could put data in the URL
  img({ src, alt }) {
    const url = typeof src === 'string' ? src : ''
    // a path on this site, but not "//host/…", which is another site
    if (/^(data:image\/|blob:|\/(?![/\\]))/.test(url)) return <img src={url} alt={alt ?? ''} className="max-w-full rounded-xl" />
    return /^https?:\/\//.test(url) ? (
      <a href={url} target="_blank" rel="noreferrer noopener">{alt || url}</a>
    ) : (
      <span>{alt}</span>
    )
  },
}

export const Markdown = memo(function Markdown({ text, className, sans }: { text: string; className?: string; sans?: boolean }) {
  const font = useContext(ReplyFont)
  return (
    <div className={clsx('prose-reply', !sans && font !== 'sans' && `reply-${font}`, className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  )
})
