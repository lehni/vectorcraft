// Text along a wave, which the mouse reshapes as it moves:
var path = new Path({
	segments: [[60, 200], [260, 120], [460, 280], [660, 200]]
});
path.smooth();
var text = new PathText(path);
text.content = 'Text runs along a path, each letter turned to where it sits.';
text.fontFamily = 'Georgia';
text.fontSize = 22;
text.words.forEach(function(word, i) {
	word.fillColor = i % 2 ? '#c04' : '#048';
});

// More text, around a circle:
var circle = new PathText(new Path.Circle({
	center: [360, 420],
	radius: 90
}));
circle.content = 'Around and around and around the circle it goes.';
circle.fontFamily = 'Georgia';
circle.fontSize = 22;

// Show the guides, which the text items don't draw:
var guides = new Group([path.clone(), circle.textPath.clone()]);
guides.strokeColor = '#ddd';
guides.sendToBack();

function onMouseMove(event) {
	path.segments[1].point.y = event.point.y;
	path.smooth();
	guides.firstChild.segments = path.segments;
}
