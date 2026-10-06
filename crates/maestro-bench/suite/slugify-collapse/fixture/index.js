function slugify(s) {
  return s.trim().toLowerCase().replace(/ /g, "-");
}

module.exports = { slugify };
