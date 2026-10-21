// Text styled by range: by word, by letter and by paragraph.
var text = new PointText({
	point: [60, 80],
	content: 'Every word its own size\nand every letter its own color\n'
			+ 'Right justified, with space before, and a note1',
	fontFamily: 'Georgia',
	fontSize: 20
});

var paragraphs = text.paragraphs;

// Walk the words of the first paragraph and restyle them:
paragraphs[0].words.forEach(function(word) {
	word.fontSize = 14 + word.content.length * 4;
});

// Walk the letters of the second paragraph:
paragraphs[1].letters.forEach(function(letter, i) {
	letter.fillColor = { hue: i * 12, saturation: 0.8, brightness: 0.8 };
});

// Paragraph properties apply to whole paragraphs:
var last = paragraphs[2];
last.justification = 'right';
last.spaceBefore = 20;
last.fontStyle = 'italic';
text.range(last.start, last.start + 5).underline = true;

// Tracking and superscripts on ranges:
text.words[1].letterTracking = 300;
var end = text.content.length;
text.range(end - 1, end).baselinePosition = 'superscript';
