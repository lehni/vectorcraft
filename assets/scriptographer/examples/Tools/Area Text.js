// Text that flows inside areas, around a circle that follows the mouse.
var words = 'Text flows inside an area, line by line, broken where the words '
		+ 'no longer fit, or at their soft hy­phens, and on into the next '
		+ 'col­umn. ';

// Two justified columns in a rectangle:
var text = new AreaText(new Rectangle(40, 40, 420, 260));
text.columns = 2;
text.gutter = 20;
text.content = words + words + words + '\n' + words + words + words;
text.fontFamily = 'Georgia';
text.fontSize = 15;
text.justification = 'justify';
text.paragraphs[1].fillColor = '#c04';

// Centered text in a circle:
var circle = new AreaText(new Path.Circle({
	center: [640, 200],
	radius: 140
}));
circle.content = words + words + words;
circle.fontFamily = 'Georgia';
circle.fontSize = 15;
circle.justification = 'center';

// A heading in an area as high as its lines need:
var heading = new AreaText(new Rectangle(40, 340, 420, 0));
heading.content = 'A heading in an area that grows to hold all of its lines';
heading.fontFamily = 'Georgia';
heading.fontSize = 28;
heading.fitHeight();

// Show the guides, which the text items don't draw:
var guides = new Group([heading.textPath.clone(), text.textPath.clone(),
		circle.textPath.clone()]);
guides.strokeColor = '#ddd';
guides.sendToBack();

// A circle above the columns, which pushes their text aside:
var wrap = new Path.Circle({
	center: [250, 170],
	radius: 50,
	fillColor: '#048',
	textWrap: { offset: 8 }
});

function onMouseMove(event) {
	wrap.position = event.point;
}
