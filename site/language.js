// GitHub Pages cannot inspect request headers. navigator.languages exposes the
// browser preferences used for Accept-Language; explicit /zh/ and /en/ links
// always keep the language the visitor chose.
(() => {
  const preferences = navigator.languages?.length
    ? navigator.languages
    : [navigator.language || "en"];
  const language = preferences
    .map((value) => value.toLowerCase().split(/[-_]/)[0])
    .find((value) => value === "zh" || value === "en") || "en";
  const target = new URL(`${language}/`, location.href);
  target.search = location.search;
  target.hash = location.hash;
  location.replace(target.href);
})();
