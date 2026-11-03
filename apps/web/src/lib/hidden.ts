// While the page is off screen, a looping animation (the mark reeling while the hub is unreachable,
// a working dot) still asks for a frame on every vsync: on a 120 Hz phone that kept the app's main
// thread at about 12% of a core until Android froze it. Marking the page lets styles.css pause them.
export function installHiddenFlag(doc: Document = document) {
  const apply = () => doc.documentElement.toggleAttribute('data-hidden', doc.visibilityState === 'hidden')
  doc.addEventListener('visibilitychange', apply)
  apply()
}
