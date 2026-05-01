const AWS = require("aws-sdk");
const moment = require("moment");

const {
  AWS_ACCESS_KEY,
  AWS_SECRET_KEY,
  AWS_BUCKET_NAME,
  AWS_REGION,
} = require("../config");

AWS.config.update({
  accessKeyId: AWS_ACCESS_KEY,
  secretAccessKey: AWS_SECRET_KEY,
  region: AWS_REGION,
});

/**
 * Standard S3 client — used for backend/server-side uploads (no acceleration).
 */
const s3 = new AWS.S3();

/**
 * Accelerated S3 client — used ONLY for generating presigned upload URLs
 * that are sent to the frontend/client.
 * Requires the bucket to have Transfer Acceleration enabled.
 */
const s3Accelerated = new AWS.S3({ useAccelerateEndpoint: true });

const cloudFront = new AWS.CloudFront();

/**
 * Returns a CloudFront URL for a given S3 file key.
 * All media served to clients MUST go through CloudFront, not directly from S3.
 *
 * @param {string} fileKey - The S3 object key
 * @returns {string|null} Full CloudFront URL
 */
const { AWS_CLOUDFRONT_URL } = require("../config");
exports.getMediaUrl = (fileKey) => {
  if (!fileKey) return null;
  if (!AWS_CLOUDFRONT_URL) return null;
  return `${AWS_CLOUDFRONT_URL}/${fileKey}`;
};

exports.generateUploadURL = async (userId) => {
  try {
    var folderDate = moment(new Date()).format("DD-MM-YYYY");
    const params = {
      Bucket: AWS_BUCKET_NAME,
      Key: `${userId}/${folderDate}`,
      // Expires: expiresInMinutes * 60, // Convert minutes to seconds
      ContentType: "application/octet-stream", // Set the appropriate content type for your file
    };

    // Use accelerated client — this URL is given to the frontend for direct upload
    const url = await s3Accelerated.getSignedUrlPromise("putObject", params);
    return url;
  } catch (error) {
    console.error("Error:", error);
    throw error;
  }
};

// Get an object from S3 via CloudFront
exports.getObjectFromS3ViaCloudFront = async (bucketName, objectKey) => {
  const signedUrl = await cloudFront.getSignedUrlPromise("getObject", {
    Bucket: bucketName,
    Key: objectKey,
    Expires: 3600, // URL expiration time in seconds
  });
  return signedUrl;
  // Perform operations using the signed URL
  // e.g., download the object using the signed URL
};

// Create an object in S3 via CloudFront
exports.createObjectInS3ViaCloudFront = async (bucketName, objectKey, data) => {
  await s3
    .putObject({
      Bucket: bucketName,
      Key: objectKey,
      Body: data,
    })
    .promise();
};

// Put an object in S3 via CloudFront
exports.putObjectInS3ViaCloudFront = async (
  bucketName,
  objectKey,
  filePath
) => {
  const fileData = require("fs").readFileSync(filePath);
  await s3
    .putObject({
      Bucket: bucketName,
      Key: objectKey,
      Body: fileData,
    })
    .promise();
};

// Get all objects from S3 via CloudFront
exports.getAllObjectsFromS3ViaCloudFront = async (bucketName) => {
  const objects = await s3.listObjectsV2({ Bucket: bucketName }).promise();
  return objects;
};

// Verify the existence of an object in S3 via CloudFront
exports.verifyObjectInS3ViaCloudFront = async (bucketName, objectKey) => {
  const headData = await s3
    .headObject({
      Bucket: bucketName,
      Key: objectKey,
    })
    .promise();
  return headData;
};
