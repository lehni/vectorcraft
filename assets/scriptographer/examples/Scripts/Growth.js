var paths = project.getItems({
	class: Path,
	selected: true
});

if (paths.length) {
	var components = {
		scale: { label: 'Scale', value: 0.95 }
	};
	var values = Dialog.prompt('Grow:', components);

	if (values) {
		var path = paths[0];
		var group = new Group([path]);
		for (var i = 0; i < 100; i++) {
			var lastCurve = path.lastCurve;
			var p2 = lastCurve.getPointAtTime(1);
			var a2 = lastCurve.getTangentAtTime(1).angle;
			var clone = path.clone();
			group.addChild(clone);
			clone.scale(values.scale);
			var firstCurve = clone.firstCurve;
			var p1 = firstCurve.getPointAtTime(0);
			var a1 = firstCurve.getTangentAtTime(0).angle;
			clone.rotate(a2 - a1, p1);
			clone.position += p2 - p1;
			path = clone;
			if (clone.length < 1) {
				clone.remove();
				break;
			}
		}
	}
} else {
	Dialog.alert('Please select a path first.');
}
