// Tellfold site. Loaded in <head> so the page knows before first paint whether scripts and motion are on; the rest
// waits for the DOM. Without this file, or with reduced motion, every section is simply there and nothing moves.
(function () {
  var root = document.documentElement;
  var still = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  root.classList.add('js');
  if (!still) root.classList.add('motion');

  document.addEventListener('DOMContentLoaded', revealOnScroll);
  document.addEventListener('DOMContentLoaded', holdFilm);
  document.addEventListener('DOMContentLoaded', stepRun);
  document.addEventListener('DOMContentLoaded', citeReport);
  document.addEventListener('DOMContentLoaded', countVisit);

  // Page views and clicks on links marked data-count (Download, Sponsor) go to Tellfold's own counter
  // (stats/ in the repo), only from the live site. It keeps a number per day, page or link, and where the visit came
  // from (a ?ref= tag or the referring site), and nothing about the visitor: no cookies, no IP address. Open the
  // page with #no-count once to stop counting your own visits in this browser; open it with #count to resume.
  var COUNTER = 'https://tellfold-stats.tellfold.workers.dev/hit';

  function countVisit() {
    if (location.hostname !== 'narekmanukyan.github.io' || navigator.webdriver || !navigator.sendBeacon) return;
    try {
      if (location.hash === '#no-count') localStorage.setItem('tellfold-no-count', '1');
      if (location.hash === '#count') localStorage.removeItem('tellfold-no-count');
      if (localStorage.getItem('tellfold-no-count')) return;
    } catch (e) { /* storage blocked: count as usual */ }

    var from = visitSource();
    hit(location.pathname, from);
    document.addEventListener('click', function (event) {
      var link = event.target.closest && event.target.closest('a[data-count]');
      if (link) hit(link.getAttribute('data-count'), from);
    });
  }

  function visitSource() {
    var tag = new URLSearchParams(location.search);
    tag = tag.get('ref') || tag.get('utm_source');
    if (tag) return tag;
    try {
      var host = new URL(document.referrer).hostname.replace(/^www\./, '');
      if (host && host !== location.hostname) return host;
    } catch (e) { /* no referrer */ }
    return 'direct';
  }

  function hit(name, from) {
    navigator.sendBeacon(COUNTER + '?n=' + encodeURIComponent(name) + '&s=' + encodeURIComponent(from));
  }

  // The film loop autoplays muted. For people who ask for less motion it never starts (CSS shows its poster instead).
  function holdFilm() {
    // Phones play the square cut (picked by <source media>); give them the matching poster too.
    if (window.matchMedia('(max-width: 759px)').matches) {
      document.querySelectorAll('video[data-poster-narrow]').forEach(function (v) {
        v.poster = v.getAttribute('data-poster-narrow');
      });
    }
    if (!still) {
      // It only plays while it is on screen.
      if (!('IntersectionObserver' in window)) return;
      var io = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          if (entry.isIntersecting) entry.target.play().catch(function () { /* autoplay refused: the poster stays */ });
          else entry.target.pause();
        });
      }, { threshold: 0.25 });
      document.querySelectorAll('video[autoplay]').forEach(function (v) { io.observe(v); });
      return;
    }
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
        entry.target.querySelectorAll('.count').forEach(function (el) { countUp(el, 300); });
        io.unobserve(entry.target);
      });
    }, { rootMargin: '0px 0px -8% 0px', threshold: 0.12 });
    items.forEach(function (el) { io.observe(el); });
  }

  // The run, round by round. Its track is several screens tall and the scene inside pins under the nav; how far the
  // reader is through the track picks the step: -1 not reached, 0 the crowd, 1 to 3 rounds 0 to 2, 4 the fold. CSS
  // does the moving; this only sets data-step and counts the mood and the totals.
  function stepRun() {
    var track = document.querySelector('[data-scene="run"]');
    if (!track || !root.classList.contains('motion')) return;
    var starts = [0.08, 0.3, 0.52, 0.74]; // how far through the pinned stretch steps 1 to 4 begin
    var moods = [null, null, '+0.40', '+0.62', '+0.55', '+0.55']; // indexed by step + 1, from the run's round_stats
    var mood = track.querySelector('.mood-val');
    var totals = track.querySelectorAll('.stats dd');
    var nav = parseFloat(getComputedStyle(root).getPropertyValue('--nav-h')) || 52;
    var step = null;
    var queued = false;

    function measure() {
      queued = false;
      var box = track.getBoundingClientRect();
      var through = (nav - box.top) / (box.height - (window.innerHeight - nav));
      var next = box.top > window.innerHeight * 0.55 ? -1 : 0;
      starts.forEach(function (at, i) { if (through >= at) next = i + 1; });
      if (next !== step) show(next);
    }

    function show(next) {
      var was = step;
      var to = moods[next + 1];
      var from = was === null ? null : moods[was + 1];
      step = next;
      track.setAttribute('data-step', String(next));
      if (!to) mood.textContent = '–';
      else if (!from) mood.textContent = to;
      else if (from !== to) countBetween(mood, parseFloat(from), to, 0);
      if (next === 4 && was !== null && was < 4) totals.forEach(function (el) { countUp(el, 1400); });
    }

    function queue() {
      if (queued) return;
      queued = true;
      requestAnimationFrame(measure);
    }
    window.addEventListener('scroll', queue, { passive: true });
    window.addEventListener('resize', queue);
    measure();
  }

  // The report excerpt. When it comes into view its citations light up one post at a time, each opening the post it
  // points to, then the audit line ticks. After that, or as soon as the reader picks one, a citation opens on click.
  function citeReport() {
    var box = document.querySelector('[data-scene="cites"]');
    if (!box) return;
    var cites = Array.prototype.slice.call(box.querySelectorAll('.cite'));
    var cards = box.querySelectorAll('.card');
    var timer = null;
    var done = false;

    function open(post) {
      cites.forEach(function (c) {
        var on = c.getAttribute('data-post') === post;
        c.classList.toggle('is-on', on);
        c.setAttribute('aria-expanded', String(on));
        if (on) c.classList.add('is-lit');
      });
      cards.forEach(function (card) { card.classList.toggle('is-on', card.getAttribute('data-post') === post); });
    }
    function finish() {
      clearTimeout(timer);
      if (done) return;
      done = true;
      cites.forEach(function (c) { c.classList.add('is-lit'); });
      box.classList.add('is-done');
      countUp(box.querySelector('.audit-n'), 0);
    }
    cites.forEach(function (cite) {
      cite.addEventListener('click', function () { finish(); open(cite.getAttribute('data-post')); });
    });

    if (!root.classList.contains('motion') || !('IntersectionObserver' in window)) {
      finish();
      open(cites[0].getAttribute('data-post'));
      return;
    }
    var posts = [];
    cites.forEach(function (c) {
      var post = c.getAttribute('data-post');
      if (posts.indexOf(post) < 0) posts.push(post);
    });
    var io = new IntersectionObserver(function (entries) {
      if (!entries[0].isIntersecting) return;
      io.disconnect();
      var i = 0;
      (function next() {
        if (done) return;
        if (i === posts.length) { timer = setTimeout(finish, 400); return; }
        open(posts[i]);
        i += 1;
        timer = setTimeout(next, 1150);
      })();
    }, { threshold: 0.35 });
    io.observe(box);
  }

  // Counts a number up from zero to the value the page already shows, in the same format.
  function countUp(el, delay) {
    if (!el || !root.classList.contains('motion')) return;
    countBetween(el, 0, el.getAttribute('data-final') || el.textContent, delay);
  }

  function countBetween(el, from, end, delay) {
    var to = parseFloat(end.replace('−', '-'));
    var places = (end.split('.')[1] || '').length;
    var sign = end.charAt(0) === '+' ? '+' : '';
    var run = (el.countRun || 0) + 1;
    var began = null;
    el.countRun = run;
    el.setAttribute('data-final', end);
    el.textContent = sign + from.toFixed(places);
    setTimeout(function () { requestAnimationFrame(frame); }, delay);

    function frame(now) {
      if (el.countRun !== run) return;
      if (began === null) began = now;
      var k = Math.min(1, (now - began) / 900);
      var eased = 1 - Math.pow(1 - k, 3);
      el.textContent = k < 1 ? sign + (from + (to - from) * eased).toFixed(places) : end;
      if (k < 1) requestAnimationFrame(frame);
    }
  }
})();
