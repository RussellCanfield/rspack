var path = require("path");

module.exports = {
	entry: "./example.js",
	context: __dirname,
	mode: "development",
	devtool: false,
	output: {
		path: path.join(__dirname, "dist"),
		publicPath: "/dist/",
	},
};
