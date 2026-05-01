const { Router } = require("express");
const postsController = require("../controllers/postsController");

const router = Router();
router.get("/", postsController.fetchAllPosts); // Get All Posts List API
router.delete("/:id", postsController.deletePost); // Delete Post List API
router.get("/:id", postsController.getPostDetails); // Get Post details API

module.exports = router;
