function csv_column(csv, name) {
  const lines = csv.split("\n").filter((l) => l.length > 0);
  if (lines.length === 0) {
    return null;
  }
  const header = lines[0].split(",");
  const idx = header.indexOf(name);
  if (idx === -1) {
    return null;
  }
  return lines.slice(1).map((l) => l.split(",")[idx]);
}

module.exports = { csv_column };
