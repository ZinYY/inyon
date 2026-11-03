let asked = false

/** Adds the serif reply face (serif.css) once, for the first session that reads in serif. */
export function loadSerifFont() {
  if (asked) return
  asked = true
  void import('../serif.css').catch(() => {})
}

/** Whether the serif face has been asked for (tests). */
export const serifAsked = () => asked
