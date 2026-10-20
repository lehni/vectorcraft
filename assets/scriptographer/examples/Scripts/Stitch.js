var paths = project.getItems({
	class: Path,
	selected: true
});
if (paths.length > 0) {
	var components = {
		distance: { value: 1, label: 'Distance' },
		size: { value: 10, label: 'Size' }
	};
	var values = Dialog.prompt('Stitch:', components);
	// Paper.js would loop forever on a distance of 0:
	if (values && values.distance > 0) {
		for (var j = 0; j < paths.length; j++) {
			var item = paths[j];
			// Scriptographer converted a clone of the path to points
			// values.distance apart (curvesToPoints()) and walked its curves.
			// Paper.js walks the path itself, values.distance at a time:
			var mul = 1;
			var res = new Path();
			for (var offset = 0, length = item.length; offset < length;
					offset += values.distance) {
				var pt = item.getPointAt(offset);
				var n = item.getNormalAt(offset);
				if (n.x != 0 || n.y != 0) {
					n = n.normalize(values.size);
					res.add(pt + (n * mul));
					mul *= -1;
				}
			}
		}
	}
} else {
	Dialog.alert('Please select a path first.');
}
