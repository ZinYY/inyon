import remarkMath from 'remark-math'
import rehypeKatex from 'rehype-katex'
import 'katex/dist/katex.min.css'

// Loaded only for replies containing math; fonts are served by Inyon itself.
export const remark = remarkMath
export const rehype = rehypeKatex
export const options = { trust: false, strict: 'ignore' as const, maxExpand: 1000, maxSize: 20 }
