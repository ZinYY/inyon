import clsx from 'clsx'
import { Search, X } from 'lucide-react'

/** A search box that narrows a list in place. Esc clears it. */
export function FilterInput({ value, onChange, placeholder, className }: { value: string; onChange: (v: string) => void; placeholder: string; className?: string }) {
  return (
    <label className={clsx('flex h-9 items-center gap-2 rounded-full border border-line bg-elev px-3 text-fg-3 shadow-card focus-within:border-line-strong', className)}>
      <Search className="size-3.5 shrink-0" />
      <input
        type="search"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => e.key === 'Escape' && value && (e.stopPropagation(), onChange(''))}
        placeholder={placeholder}
        aria-label={placeholder}
        className="min-w-0 flex-1 bg-transparent text-[13.5px] text-fg placeholder:text-fg-3 [&::-webkit-search-cancel-button]:hidden"
      />
      {value && (
        <button type="button" onClick={() => onChange('')} aria-label="Clear" className="-mr-1 flex size-6 shrink-0 items-center justify-center rounded-full hover:bg-hover hover:text-fg">
          <X className="size-3.5" />
        </button>
      )}
    </label>
  )
}
