// Tellfold site. Loaded in <head> so the page knows before first paint whether scripts and motion are on; the rest
// waits for the DOM. Without this file, or with reduced motion, every section is simply there and nothing moves.
(function () {
  var root = document.documentElement;
  var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  root.classList.add('js');
  if (!still) root.classList.add('motion');

  document.addEventListener('DOMContentLoaded', revealOnScroll);
  document.addEventListener('DOMContentLoaded', holdFilm);
  document.addEventListener('DOMContentLoaded', countVisit);

  // Visits are counted by GoatCounter, without cookies and only on the live site. Clicks on links marked
  // data-count (Download, Sponsor) count as events, which shows how many visits end in a download. Open the page
  // with #toggle-goatcounter once to stop counting your own visits in this browser; open it again to resume.
  var COUNTER = 'https://tellfold.goatcounter.com/count';

  function countVisit() {
    if (location.hostname !== 'narekmanukyan.github.io' || navigator.webdriver) return;
    try {
      if (location.hash === '#toggle-goatcounter') {
        if (localStorage.getItem('skipgc') === 't') localStorage.removeItem('skipgc');
        else localStorage.setItem('skipgc', 't');
      }
      if (localStorage.getItem('skipgc') === 't') return;
    } catch (e) { /* storage blocked: count as usual */ }

    hit({ p: location.pathname, t: document.title, r: document.referrer, q: location.search, s: screen.width });
    document.addEventListener('click', function (event) {
      var link = event.target.closest && event.target.closest('a[data-count]');
      if (link) hit({ p: link.getAttribute('data-count'), t: link.textContent.trim(), e: 'true' });
    });
  }

  function hit(vars) {
    vars.rnd = Math.random().toString(36).slice(2, 7);
    var url = COUNTER + '?' + Object.keys(vars).filter(function (k) { return vars[k] !== '' && vars[k] != null; })
      .map(function (k) { return encodeURIComponent(k) + '=' + encodeURIComponent(vars[k]); }).join('&');
    if (!navigator.sendBeacon || !navigator.sendBeacon(url)) new Image().src = url;
  }

  // The film loop autoplays muted. For people who ask for less motion it never starts (CSS shows its poster instead).
  function holdFilm() {
    // Phones play the square cut (picked by <source media>); give them the matching poster too.
    if (window.matchMedia('(max-width: 759px)').matches) {
      document.querySelectorAll('video[data-poster-narrow]').forEach(function (v) {
        v.poster = v.getAttribute('data-poster-narrow');
      });
    }
    if (!still) return;
    document.querySelectorAll('video[autoplay]').forEach(function (v) {
      v.removeAttribute('autoplay');
      v.preload = 'none';
      v.pause();
    });
  }

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
