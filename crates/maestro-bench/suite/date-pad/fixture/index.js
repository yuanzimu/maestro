function format_date(d) {
  const y = d.getFullYear();
  const m = d.getMonth() + 1;
  const day = d.getDate();
  return `${y}-${m}-${day}`;
}

module.exports = { format_date };
