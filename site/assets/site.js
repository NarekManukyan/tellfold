// Tellfold site. Loaded in <head> so the page knows before first paint whether scripts and motion are on; the rest
// waits for the DOM. Without this file, or with reduced motion, every section is simply there and nothing moves.
(function () {
  var root = document.documentElement;
  var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  root.classList.add('js');
  if (!still) root.classList.add('motion');

  document.addEventListener('DOMContentLoaded', revealOnScroll);

  function revealOnScroll() {
    var items = document.querySelectorAll('.reveal');
    if (!root.classList.contains('motion') || !('IntersectionObserver' in window)) {
      items.forEach(function (el) { el.classList.add('in'); });
      return;
    }
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        entry.target.classList.add('in');
        io.unobserve(entry.target);
      });
    }, { rootMargin: '0px 0px -8% 0px', threshold: 0.12 });
    items.forEach(function (el) { io.observe(el); });
  }
})();
